import { useState, useEffect, useRef, useCallback, useMemo } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { cn } from "@/lib/utils";
import { type IconName } from "@/components/SvgIcon";
import { parseShortcut, matchShortcut } from "./shortcut";
import { AsrEditor, type AsrEditorHandle } from "./AsrEditor";
import { parseSegments, type Segment } from "./hotwords";
import { Toolbar, type ToolDef } from "./Toolbar";
import { TranslationPane } from "./TranslationPane";
import {
  type TranslateMode,
  resolveRememberedTranslateMode,
  parseThrottleSeconds,
  buildTranslatePopupItems,
} from "./translateMode";
import { useT, t as ti18n } from "@/lib/i18n";
import { InstantView } from "./InstantView";

const POLISH_MODES = [0, 1, 2];
const DENOISE_MODES = [0, 1, 2];

/** 翻译下拉菜单中「关闭翻译」项的 name 标记值（区别于 TranslateMode 类型）。 */
const TRANSLATE_OFF_SENTINEL = "__off__";

interface ToolbarState {
  polishMode: number;
  denoiseMode: number;
  polishLlmValid: boolean;
  hideToolbar: boolean;
  editShortcut: string;
  translateMode: string;
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
    polishMode: 0, denoiseMode: 1, polishLlmValid: false,
    hideToolbar: true, editShortcut: "CmdOrCtrl+Enter",
    translateMode: "manual",
  });
  const [popupType, setPopupType] = useState<PopupType>(null);
  const [popupItems, setPopupItems] = useState<PopupItem[]>([]);
  const [toast, setToast] = useState<string | null>(null);
  const [toastError, setToastError] = useState(false);
  const POLISH_ERROR_TIMEOUT_MS = 1500;
  const [polishLoading, setPolishLoading] = useState(false);
  const [polishError, setPolishError] = useState<string | null>(null);
  const [translateMode, setTranslateMode] = useState<TranslateMode>('off');
  const [translatedText, setTranslatedText] = useState("");
  const [translating, setTranslating] = useState(false);
  // record-mode 切换：toggle（CM6 编辑器视图）/ instant（PTT/hands-free 指示卡视图）
  const [recordMode, setRecordMode] = useState<"toggle" | "instant">("toggle");
  // instant-state：{ state, text }——供 InstantView 渲染
  const [instantState, setInstantState] = useState("");
  const [instantText, setInstantText] = useState("");
  // segments：后端 segments_json 解析（含 hotwords 候选）。null = 无段信息（降级扁平 text）。
  const [segments, setSegments] = useState<Segment[] | null>(null);
  // recordMode 的 ref——update-result handler 在 [] effect 内注册，闭包捕获旧 recordMode，
  // 读 ref 避免 React 闭包陷阱（对齐 translateModeRef / toolbarVisibleRef 模式）。
  // 用于 instant 模式把流式 partial 也喂给 InstantView（实时文字显示）。
  const recordModeRef = useRef<"toggle" | "instant">("toggle");
  useEffect(() => { recordModeRef.current = recordMode; }, [recordMode]);

  const asrEditorRef = useRef<AsrEditorHandle>(null);
  const caretRef = useRef<number | null>(null);
  const [asrEditorResetKey, setAsrEditorResetKey] = useState(0);
  const speakingTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  // 第十四轮 P3-7：polish-error setTimeout 用 ref 管理（原无 ref，unmount 泄漏 + 连续错误被截短）
  const polishErrorTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
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

  // 第十二轮 P3-1：toast timer 用 ref 管理——连续不同 ms 的 toast，前次 timer 未到期时
  // 新 toast 的 timer 并行跑，早到的会截短后到的（如 3000ms 后接 5000ms，3000ms 到期清掉后者）。
  // 对齐 Settings/index.tsx toastTimerRef 范式。
  const toastTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const showToast = useCallback((msg: string, ms = 2000, isError = false) => {
    if (toastTimerRef.current) {
      clearTimeout(toastTimerRef.current);
      toastTimerRef.current = null;
    }
    setToast(msg);
    setToastError(isError);
    toastTimerRef.current = setTimeout(() => { setToast(null); setToastError(false); }, ms);
  }, []);

  // 第十四轮 P3-7：unmount 清理所有 timer ref（speakingTimer / polishErrorTimer / toastTimer）
  // 防 leak（原 speakingTimer / polishErrorTimer 无 unmount cleanup）。
  useEffect(() => () => {
    if (speakingTimer.current) clearTimeout(speakingTimer.current);
    if (polishErrorTimerRef.current) clearTimeout(polishErrorTimerRef.current);
    if (toastTimerRef.current) clearTimeout(toastTimerRef.current);
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
      if (st.hideToolbar === false) {
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
        setIsSpeaking((prev) => {
          if (!prev) void invoke("perf_log_cmd", { msg: "[FE] isSpeaking false -> true" });
          return true;
        });
      } else {
        if (speakingTimer.current) clearTimeout(speakingTimer.current);
        speakingTimer.current = setTimeout(() => {
          setIsSpeaking((prev) => {
            if (prev) void invoke("perf_log_cmd", { msg: "[FE] isSpeaking true -> false (200ms debounce)" });
            return false;
          });
        }, 200);
      }
    });
    // agent task 错误通知（录音进行中时 StartAgentRecording 被拒等）
    const unlistenAgentError = listen<string>("agent-task://error", (payload) => {
      const msg = typeof payload === "string" ? payload : (payload as any)?.payload ?? "";
      if (msg) showToast(msg, 3000);
    });
    // 麦克风不可用错误——红色气泡提示
    const unlistenMicError = listen<string>("mic-error", (payload) => {
      const msg = typeof payload === "string" ? payload : (payload as any)?.payload ?? "";
      if (msg) showToast(msg, 5000, true);
    });
    // AX 权限未授权——提示授权+重启（识别窗依赖 AX 权限 show/focus）
    const unlistenPermission = listen<string>("permission-required", (payload) => {
      const perm = typeof payload === "string" ? payload : (payload as any)?.payload ?? "";
      if (perm === "accessibility") {
        showToast(t("result.axPermissionRequired"), 8000, true);
      }
    });
    return () => { unlisten.then((f) => f()); unlistenAgentError.then((f) => f()); unlistenMicError.then((f) => f()); unlistenPermission.then((f) => f()); };
  }, []);

  // ── Toolbar hover ──
  useEffect(() => {
    const container = document.getElementById("result-container");
    if (!container) return;
    if (toolbarState.hideToolbar === false) return;

    const onMove = () => showToolbar();
    const onLeave = () => hideToolbar();
    container.addEventListener("mousemove", onMove);
    container.addEventListener("mouseleave", onLeave);
    return () => {
      container.removeEventListener("mousemove", onMove);
      container.removeEventListener("mouseleave", onLeave);
    };
  }, [toolbarState.hideToolbar, showToolbar, hideToolbar]);

  // ── Tauri events ──
  useEffect(() => {
    let unlistens: UnlistenFn[] = [];
    let cancelled = false;

    (async () => {
      const handlers: [string, (payload: unknown) => void][] = [
        ["show-result", (p) => {
          // payload: { text, segments }（2026-08-02 hotwords 下拉）。segments 可为 null。
          const payload = p as { text: string; segments?: string | null } | string;
          // 兼容：极旧逻辑可能传 bare string，统一提取 text/segments。
          const t = typeof payload === "string" ? payload : payload.text;
          const segJson = typeof payload === "string" ? null : (payload.segments ?? null);
          const isPlaceholder = t === "正在聆听…" || t === "正在聆听..." || t === "Listening…" || t === "Listening...";
          setVisible(true);
          setIsRecording((prev) => {
            if (!prev) void invoke("perf_log_cmd", { msg: "[FE] isRecording false -> true (show-result)" });
            return true;
          });
          // toggle 会话开始：清 instant 视图残留状态（上次 PTT/hands-free 的指示内容）
          setInstantState("");
          setInstantText("");
          setTranslateMode('off');
          setTranslatedText("");
          translatingRef.current = false;
          setTranslating(false);
          invoke("set_translation_active", { active: false });
          refreshActive();
          if (isPlaceholder) {
            setText("");
            setSegments(null);
            caretRef.current = null;
            setAsrEditorResetKey(k => k + 1);
          } else {
            setText(t);
            setSegments(parseSegments(segJson));
            caretRef.current = null;
          }
        }],
        ["update-result", (p) => {
          const payload = p as { text: string; insertion: boolean; caret: number; segments?: string | null };
          caretRef.current = payload.insertion ? payload.caret : null;
          setText(payload.text);
          setSegments(parseSegments(payload.segments));
          // instant 模式：流式 partial 也喂给 InstantView（实时文字显示）。
          // 读 recordModeRef 避免 React 闭包陷阱（handler 在 [] effect 注册）。
          // toggle 模式不写——InstantView 被 display:none 隐藏，无需更新。
          if (recordModeRef.current === "instant") {
            setInstantText(payload.text);
          }
        }],
        ["clear-result", () => {
          setText("");
          setSegments(null);
          caretRef.current = null;
          setVisible(false);
          setIsRecording((prev) => {
            if (prev) void invoke("perf_log_cmd", { msg: "[FE] isRecording true -> false (clear-result)" });
            return false;
          });
          setAsrEditorResetKey(k => k + 1);
        }],
        ["hide-result", () => {
          setVisible(false);
          setSegments(null);
          setIsRecording((prev) => {
            if (prev) void invoke("perf_log_cmd", { msg: "[FE] isRecording true -> false (hide-result)" });
            return false;
          });
          setAsrEditorResetKey(k => k + 1);
        }],
        ["config-changed", () => refreshActive()],
        ["polish-done", () => setPolishLoading(false)],
        ["polish-started", () => setPolishLoading(true)],
        ["polish-error", (msg) => {
          setPolishLoading(false);
          setPolishError(typeof msg === "string" ? msg : "润色失败");
          // 第十四轮 P3-7：ref 管理 timer（防泄漏 + 连续错误被截短）
          if (polishErrorTimerRef.current) clearTimeout(polishErrorTimerRef.current);
          polishErrorTimerRef.current = setTimeout(() => setPolishError(null), POLISH_ERROR_TIMEOUT_MS);
        }],
        ["prepare-record", (p) => {
          invoke("start_recording", {
            prepareId: p as number,
            selection: null,
          });
        }],
        // flush-edit：后端停止录音/润色前强制 commit 编辑器内容（防抖未提交的编辑）。
        // 前端 commit（清防抖 timer + 提交）后回传 edit_flushed 通知后端继续。
        ["flush-edit", (flushId) => {
          asrEditorRef.current?.commit();
          void invoke("edit_flushed", { flushId: flushId as number });
        }],
        // record-mode：切 AsrWindow 视图（toggle 编辑器 ↔ instant 指示卡）。payload 是纯字符串。
        ["record-mode", (p) => {
          const mode = p as string;
          if (mode === "toggle" || mode === "instant") setRecordMode(mode);
        }],
        // instant-state：{ state, text }——驱动 InstantView 四态指示。
        ["instant-state", (p) => {
          const payload = p as { state: string; text: string };
          setInstantState(payload?.state ?? "");
          setInstantText(payload?.text ?? "");
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
  const doTranslate = useCallback(async () => {
    const source = asrEditorRef.current?.getText() ?? text;
    if (!source.trim()) return;
    if (translatingRef.current) return;
    translatingRef.current = true;
    setTranslating(true);
    lastTranslatedRef.current = source;
    try {
      // targetType: "result" → 走旧事件名 translate-progress|done（与 CompactEditor 隔离）
      // 2026-07-17 修复发现 6：CompactEditor 翻译事件改新事件名，不再泄漏到 Result 窗口
      await invoke("translate_text", { text: source, targetType: "result" });
    } catch (e) {
      translatingRef.current = false;
      setTranslating(false);
      showToast(ti18n("result.translateFail") + String(e));
    }
  }, [text, showToast]);
  useEffect(() => { doTranslateRef.current = doTranslate; }, [doTranslate]);

  const enterTranslateMode = useCallback(() => {
    const mode = resolveRememberedTranslateMode(toolbarState.translateMode);
    setTranslateMode(mode);
    if (!expanded) {
      setExpanded(true);
      invoke("set_result_click_through", { expanded: true });
    }
    setTimeout(() => {
      if (translateModeRef.current !== 'off') doTranslateRef.current();
    }, 100);
    invoke("set_translation_active", { active: true });
  }, [toolbarState.translateMode, expanded]);

  // 翻译事件监听——仅翻译模式开启/关闭时订阅/退订，档位切换不重订阅（防竞态丢失 translate-done）
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
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [translateMode !== 'off']);

  // 自动档节流——translateMode 为 4s/8s/12s 时启动定时器
  useEffect(() => {
    const secs = parseThrottleSeconds(translateMode);
    if (secs === null) return;

    const timer = setInterval(() => {
      const current = asrEditorRef.current?.getText() ?? "";
      if (current !== lastTranslatedRef.current && !translatingRef.current && current.trim()) {
        // 第十四轮 P1-1：用 ref 而非闭包 doTranslate——流式 ASR 高频 setText → text 变 →
        // doTranslate 新引用 → 本 effect 重起 → clearInterval+新 timer → 4s/8s/12s 跑不满。
        // doTranslateRef 在 :332 同步，:412 keydown 已正确用 ref，此处漏用。
        doTranslateRef.current();
      }
    }, secs * 1000);

    return () => clearInterval(timer);
  }, [translateMode]); // 第十四轮 P1-1：移除 doTranslate（改用 ref，避免流式打断 timer）

  const onSave = useCallback(() => {
    asrEditorRef.current?.commit();
  }, []);

  const toggleExpand = useCallback(() => {
    const next = !expanded;
    setExpanded(next);
    invoke("set_result_click_through", { expanded: next });
  }, [expanded]);

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
      const sc = parseShortcut(toolbarState.editShortcut);
      if (matchShortcut(e, sc)) {
        e.preventDefault();
        asrEditorRef.current?.commit();
      }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [popupType, toolbarState.editShortcut, win]);

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
      label: polishLabels[m], current: m === toolbarState.polishMode, mode: m,
    })));
    setPopupType("polish");
  };

  const openDenoisePopup = async () => {
    if (popupType === "denoise") { setPopupType(null); return; }
    const denoiseLabels = [ti18n("result.denoise.none"), ti18n("result.denoise.light"), ti18n("result.denoise.deep")];
    setPopupItems(DENOISE_MODES.map(m => ({
      label: denoiseLabels[m], current: m === toolbarState.denoiseMode, mode: m,
    })));
    setPopupType("denoise");
  };

  const openTranslatePopup = async () => {
    if (translateMode === 'off') {
      enterTranslateMode();
      return;
    }
    if (popupType === "translate") { setPopupType(null); return; }
    // 关闭翻译项 + 四档（平铺，无分隔线）
    const modeItems = buildTranslatePopupItems(translateMode, (m) =>
      m === 'manual' ? t("result.translateManual")
        : m === '4s' ? t("result.translateAuto4")
        : m === '8s' ? t("result.translateAuto8")
        : t("result.translateAuto12")
    );
    setPopupItems([
      { label: t("result.translateClose"), current: false, name: TRANSLATE_OFF_SENTINEL },
      ...modeItems,
    ]);
    setPopupType("translate");
  };

  const handlePopupSelect = async (item: PopupItem) => {
    try {
      if (popupType === "polish" && item.mode !== undefined) {
        await invoke("set_polish_mode", { mode: item.mode });
      } else if (popupType === "denoise" && item.mode !== undefined) {
        await invoke("set_denoise_mode", { mode: item.mode });
      } else if (popupType === "translate" && item.name) {
        if (item.name === TRANSLATE_OFF_SENTINEL) {
          setTranslateMode('off');
          setTranslatedText("");
          translatingRef.current = false;
          setTranslating(false);
          invoke("set_translation_active", { active: false });
          setExpanded(false);
          invoke("set_result_click_through", { expanded: false });
          setPopupType(null);
          return;
        }
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

  const tools: ToolDef[] = [
    { id: "close", icon: "close", label: t("result.close"), onClick: () => invoke("discard_recording") },
    { id: "denoise", icon: "denoise", label: t("result.denoiseMode"), active: toolbarState.denoiseMode !== 0, onClick: openDenoisePopup },
    { id: "polish", icon: "polish", label: t("result.polishMode"), active: toolbarState.polishMode !== 0, onClick: openPolishPopup },
    { id: "polish-now", icon: "polish-now", label: t("result.polishNow"), disabled: polishLoading, onClick: polishNow },
    { id: "translate", icon: "translate", label: t("result.translate"), active: translateMode !== 'off', onClick: openTranslatePopup },
    { id: "translate-now", icon: "redo", label: t("result.translateNow"), disabled: translating || translateMode === 'off', onClick: doTranslate },
    { id: "toggle-size", icon: (expanded ? "minimize" : "expand-edit") as IconName, label: expanded ? t("result.zoomOut") : t("result.zoomIn"), disabled: translateMode !== 'off', onClick: toggleExpand },
    { id: "save", icon: "save" as IconName, label: t("result.save"), onClick: onSave },
  ];

  return (
    <div className="asr-window-root relative w-full h-full">
    {/* toggle 视图：CM6 编辑器 + toolbar（display:none 切换，保留 CM6 state 不卸载） */}
    <div style={{ display: recordMode === "toggle" ? "block" : "none" }} className="relative w-full h-full">
    {/* 润色失败气泡 */}
    {polishError && (
      <div className="absolute top-2 left-1/2 -translate-x-1/2 z-50 px-3 py-1.5 rounded-md bg-red-500 text-white text-xs shadow-lg animate-pulse">
        ⚠ 润色失败：{polishError}
      </div>
    )}
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
        <Toolbar
          tools={tools}
          opacityClass={toolbarState.hideToolbar === false || toolbarVisible ? "opacity-100" : "opacity-0"}
          onDragStart={onDragStart}
        />
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
              segments={segments}
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
                segments={segments}
                caret={caretRef.current}
                expanded={true}
                onCommit={(payload) => {
                  setText(payload.text);
                  invoke("commit_edit", {
                    text: payload.text,
                    dirtyRanges: payload.dirtyRanges,
                    hasEdited: payload.hasEdited,
                    caret: payload.caret ?? null,
                    selection: payload.selection ?? null,
                  });
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
        <div className={cn(
          "absolute bottom-1.5 left-1/2 -translate-x-1/2 text-white text-xs px-2.5 py-1 rounded-md z-20 pointer-events-none",
          toastError ? "bg-red-500/90" : "bg-black/78",
        )}>
          {toast}
        </div>
      )}
    </div>
    </div>
    {/* instant 视图：PTT/hands-free 指示卡（display:none 切换） */}
    <div style={{ display: recordMode === "instant" ? "block" : "none" }}>
      <InstantView state={instantState} text={instantText} />
    </div>
    </div>
  );
}

export default Result;
