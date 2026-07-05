import { useState, useEffect, useRef, useCallback, useMemo } from "react";
import { flushSync } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { cn } from "@/lib/utils";
import { SvgIcon, type IconName } from "@/components/SvgIcon";
import { codePointOffsetTo, codePointOffsetBefore, measureCaretPx, placeCaretAtCodePoint } from "./caret";

const DIVERTED_DELAY_MS = 300;

// 编辑框双模式尺寸：窗口物理固定 720×480（setSize 在 transparent 无边框窗被 NSWindow
// 拒绝，改用 CSS 伪装），精简态只渲染顶部 520×116 小条、下方透明区点击穿透（后端轮询）。

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
    hide_toolbar: true, edit_shortcut: "CmdOrCtrl+Enter",
  });
  const [popupType, setPopupType] = useState<PopupType>(null);
  const [popupItems, setPopupItems] = useState<PopupItem[]>([]);
  const [toast, setToast] = useState<string | null>(null);
  const [polishLoading, setPolishLoading] = useState(false);
  // 光标 char offset（code-point 计数，与后端 Rust char 对齐）。null = 文本末尾。
  // 点击中间 → setCaretPos(offset)；中插态每 tick 由后端 caret（=已插入字数累加）驱动右移，
  // 故光标始终跟在最后插入的文字后；非中插态回 null（末尾）。
  const [caretPos, setCaretPos] = useState<number | null>(null);
  // caretPos 的 ref 镜像：enterEdit 需在 setCaretPos(null) 前读到进入前的点击位（恢复编辑态光标），
  // 用 ref 取最新值避免 enterEdit 闭包 stale（enterEdit deps 仅 showToolbar）。
  const caretPosRef = useRef(caretPos);
  useEffect(() => { caretPosRef.current = caretPos; }, [caretPos]);

  const textRef = useRef<HTMLDivElement>(null);
  const editingRef = useRef(false);
  const displayedRef = useRef("");
  const pendingDiverted = useRef<string | null>(null);
  const divertedTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const editBufTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const speakingTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const toolbarVisibleRef = useRef(false);
  const editSnapshotRef = useRef(""); // 编辑前原始文本快照
  // 是否自动跟随底部（流式追加时滚到底）。用户手动上滚 → false（停止跟随，便于查看历史）；
  // 滚回底部附近（距底 < 24px）→ true（恢复跟随）。新录音重置为 true。
  const stickToBottomRef = useRef(true);
  // 滚底 rAF 句柄：流式高频 tick（100-200ms）时去重合并到单帧，避免每 tick 排一个 rAF；
  // 且延迟到 React commit + DOM 更新后再读 scrollHeight（同步读是旧文本高度，会滞后一帧）。
  const rafScrollRef = useRef<number | null>(null);

  // 经 useMemo 稳定：getCurrentWindow() 每次返回新包装对象，若写在渲染体里会让依赖 [win]
  // 的 effect 每次 re-render 都重跑。
  const win = useMemo(() => getCurrentWindow(), []);

  useEffect(() => { editingRef.current = editing; }, [editing]);
  useEffect(() => { toolbarVisibleRef.current = toolbarVisible; }, [toolbarVisible]);
  // 卸载时取消待执行的滚底 rAF（避免对已卸载 textRef 操作）。
  useEffect(() => () => {
    if (rafScrollRef.current != null) cancelAnimationFrame(rafScrollRef.current);
  }, []);

  const showToast = useCallback((msg: string, ms = 2000) => {
    setToast(msg);
    setTimeout(() => setToast(null), ms);
  }, []);

  const showToolbar = useCallback(() => {
    if (toolbarVisibleRef.current) return;
    setToolbarVisible(true);
  }, []);

  const hideToolbar = useCallback(() => {
    if (!toolbarVisibleRef.current || editingRef.current) return;
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
    // 同步写 DOM（绕过 React commit）：textRef 是 contentEditable div，React 对其 children 的
    // commit 不更新 DOM（保护用户编辑，flushSync 强制 commit 也无效）→ 文字滞后/空白。imperative
    // 写 textContent 强制 DOM = state，文字立即渲染。仅非编辑态写（编辑态 DOM 由用户控制）。
    if (textRef.current && !editingRef.current && textRef.current.textContent !== newText) {
      textRef.current.textContent = newText;
    }
    // setText 驱动 state（CaretBlink text prop + effect 重测）；flushSync 强制同步 commit，避免
    // state 被 scheduler 延迟（否则 CaretBlink effect 滞后 → 光标滞后）。
    flushSync(() => {
      setText(newText);
    });
    // rAF 延迟滚底到 DOM 更新后（读 scrollHeight 才是新文本高度，修同步读滞后一帧；
    // 且移出 setText 同步路径，与同帧 CaretBlink measure 不叠加成 layout thrashing）。
    // rafScrollRef 去重：高频 tick 合并到单帧一次。
    if (rafScrollRef.current == null) {
      rafScrollRef.current = requestAnimationFrame(() => {
        rafScrollRef.current = null;
        if (stickToBottomRef.current && textRef.current) {
          textRef.current.scrollTop = textRef.current.scrollHeight;
        }
      });
    }
  }, []);

  // VAD 驱动波纹：后端 emit("update-speaking", bool) → 有语音即亮，静音即灭
  useEffect(() => {
    const unlisten = listen<boolean>("update-speaking", (payload) => {
      // payload 可能是 boolean 或 { payload: boolean }（Tauri 版本差异），统一提取
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
          const text = p as string;
          const isPlaceholder = text === "正在聆听…" || text === "正在聆听...";
          setVisible(true);
          setIsRecording(true);
          refreshActive();
          if (isPlaceholder) {
            // 新录音开始：清空上次残留 + 光标回到末尾（null）+ 恢复底部跟随
            setText("");
            displayedRef.current = "";
            pendingDiverted.current = null;
            setCaretPos(null);
            stickToBottomRef.current = true;
            if (divertedTimer.current) { clearTimeout(divertedTimer.current); divertedTimer.current = null; }
          } else {
            // 最终文本到达：清掉可能 pending 的 diverted 延迟，否则 300ms 后定时器回调会用旧暂存
            // 纠正文本覆盖刚渲染的最终结果（录音在 diverted 300ms 内结束的竞态）。
            if (divertedTimer.current) { clearTimeout(divertedTimer.current); divertedTimer.current = null; }
            pendingDiverted.current = null;
            renderResultNow(text);
          }
        }],
        ["update-result", (p) => {
          if (editingRef.current) return;
          // payload { text, insertion, caret }：insertion=true 中插态，caret = 光标 char 偏移（随插入增长）。
          const payload = p as { text: string; insertion: boolean; caret: number };
          const newText = payload.text;
          const insertion = payload.insertion;
          if (newText === displayedRef.current || newText === pendingDiverted.current) return;
          // 光标定位：中插态跟后端 caret（每插一字光标右移一字）；非中插态（末尾追加/diverted）回末尾。
          setCaretPos(insertion ? payload.caret : null);
          // 插入态（光标在中间）或纯追加（startsWith）：立即渲染（跳过 diverted 300ms 延迟）。
          if (insertion || newText.startsWith(displayedRef.current)) {
            if (divertedTimer.current) { clearTimeout(divertedTimer.current); divertedTimer.current = null; }
            pendingDiverted.current = null;
            renderResultNow(newText);
          } else {
            // diverted（光标在末尾 + 引擎纠正早前文本）：300ms 延迟整体替换。
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
          // 清理 diverted 延迟计时器：300ms 内取消/丢弃录音时，pending 定时器若不清，到期仍会回调
          // renderResultNow 把已废弃的纠正文本写回 text/displayedRef（state 污染 + 违背 clear 语义）。
          if (divertedTimer.current) { clearTimeout(divertedTimer.current); divertedTimer.current = null; }
          pendingDiverted.current = null;
          setText("");
          displayedRef.current = "";
          setCaretPos(null);
          setVisible(false);
          setIsRecording(false);
        }],
        ["hide-result", () => {
          // 同 clear-result：隐藏窗时丢弃 pending diverted，避免后台定时器到期写脏 state。
          if (divertedTimer.current) { clearTimeout(divertedTimer.current); divertedTimer.current = null; }
          pendingDiverted.current = null;
          setVisible(false);
          setIsRecording(false);
        }],
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
        // 冷启动主动拉取一次工具栏配置（edit_shortcut / polish_mode / denoise_mode 等），
        // 不依赖 show-result（录音开始）或 config-changed（设置改动）才同步——否则从挂载到
        // 首次这两事件之间，窗口内 keydown 监听器读的是 edit_shortcut 初始默认值，用户
        // 自定义快捷键在此窗口期内不生效。防御性初始化：当前窗口可见即聚焦的路径（录音 /
        // global-edit）多已间接触发 refreshActive，但就绪即拉取消除对该副作用的隐式依赖。
        refreshActive();
      }
    })();

    return () => { cancelled = true; unlistens.forEach((fn) => fn()); };
  }, [renderResultNow, refreshActive]);

  // ── Edit mode ──
  const enterEdit = useCallback(() => {
    if (editingRef.current) return;
    if (!displayedRef.current.trim()) return;
    // 进入编辑前捕获点击位（null=无点击/拖选 → 末尾兜底）；必须在 setCaretPos(null) 前取，
    // 否则编辑态光标无条件落末尾，长文本下用户得重新找刚才要改的位置。
    const restorePos = caretPosRef.current;
    editSnapshotRef.current = displayedRef.current; // 保存快照
    setEditing(true);
    // 同步置 ref：editingRef 靠 useEffect 在 commit 后才更新，setEditing(true) 到 commit 间有窗口；
    // 此间若 update-result 到达、editingRef 仍 false → renderResultNow 写 textContent 覆盖刚进入
    // 的编辑框、打断光标。invoke("enter_edit_mode") 往返期间后端也可能还在推 update-result。
    editingRef.current = true;
    setIsRecording(false);
    setCaretPos(null); // 进入编辑态：光标位失效（交由 DOM 选区控制）
    showToolbar();
    invoke("enter_edit_mode");
    setTimeout(() => {
      const el = textRef.current;
      if (!el) return;
      el.focus();
      const sel = window.getSelection();
      sel?.removeAllRanges();
      // 恢复进入前的点击位（非编辑态点击定位过）；restorePos=null 或定位失败 → 置末尾兜底
      if (restorePos != null && placeCaretAtCodePoint(el, restorePos)) return;
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
    // textRef 的 key 随 editing 切换，退出编辑时 view 会重新挂载；必须把编辑结果写回 text
    // state，否则重挂后会回退到进入编辑前的旧 text。
    displayedRef.current = editedText;
    setText(editedText);
    setEditing(false);
    // 同步放开守护：退出编辑即恢复接收 update-result，不等 commit 后的 useEffect（否则会
    // 误拦退出后第一帧的流式更新）。
    editingRef.current = false;
    setCaretPos(null); // 退出编辑态：光标位失效，待下次 measure 重建
    invoke("commit_edit", { text: editedText });
  }, []);

  const cancelEdit = useCallback(() => {
    if (!editingRef.current) return;
    if (editBufTimer.current) clearTimeout(editBufTimer.current);
    const original = editSnapshotRef.current;
    setEditing(false);
    editingRef.current = false; // 同步放开守护（同 commitEdit）
    setCaretPos(null); // 退出编辑态：光标位失效，待下次 measure 重建
    // 恢复 contentEditable DOM 到编辑前文本
    displayedRef.current = original;
    setText(original);
    if (textRef.current) {
      textRef.current.innerText = original;
    }
    // 只退出编辑态，不 commit（不写 edited 段到 DB）
    invoke("exit_edit_without_commit");
  }, []);

  const toggleEdit = useCallback(() => {
    editingRef.current ? commitEdit() : enterEdit();
  }, [commitEdit, enterEdit]);

  // polishLoading 的 ref 镜像：polishNow 的幂等门控改读 ref，使其 useCallback 引用不随
  // polishLoading 变化——否则 global-polish-trigger 的 listen 每次 polishLoading 翻转都
  // 注销重建（每次润色开始/结束各一次）。state polishLoading 保留（按钮 disabled 用）。
  const polishLoadingRef = useRef(false);
  useEffect(() => { polishLoadingRef.current = polishLoading; }, [polishLoading]);

  // 立即润色：工具栏按钮 + 全局 polish_global_shortcut 共用。
  // polishLoadingRef 门控（幂等，与按钮 disabled 一致）+ 空文本判空（无结果静默）。
  const polishNow = useCallback(async () => {
    if (polishLoadingRef.current) return;
    if (!displayedRef.current.trim()) return;
    setPolishLoading(true);
    try { await invoke("polish_now"); showToast("润色中…"); }
    catch (e) { setPolishLoading(false); showToast("润色失败：" + e); }
  }, [showToast]);

  // 放大/缩小开关：纯 CSS 切换（窗口物理固定 720×480，setSize 在 transparent 无边框窗
  // 被 NSWindow 拒绝，无法运行时改尺寸）。精简态容器缩为顶部 520×116 小条、下方透明区
  // 由后端轮询点击穿透；长篇态容器撑满 720×480。仅通知后端切换穿透模式。
  const toggleExpand = useCallback(() => {
    const next = !expanded;
    setExpanded(next);
    invoke("set_result_click_through", { expanded: next }); // 长篇(next)=整窗可交互、精简=穿透
  }, [expanded]);

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

  // ── 非编辑态鼠标释放文本：拖选→选中替换；纯点击→光标定位 ──
  // mouseup 时选区才完整。caretRangeFromPoint 是非标准 API（Chromium 有），webkit 支持。
  // 拖选（非折叠）→ 算 [start,end) code-point 范围 + 隐藏闪烁光标（交浏览器原生高亮）+
  //   invoke set_selection（后端记录待删，首个 delta 到达时删旧插新）。
  // 纯点击（折叠）→ caretRangeFromPoint 定位 + invoke set_caret（普通中插）。
  const handleTextMouseUp = (e: React.MouseEvent) => {
    const el = textRef.current;
    if (!el || !text) return;
    const sel = window.getSelection();
    if (sel && !sel.isCollapsed && sel.rangeCount > 0) {
      const range = sel.getRangeAt(0);
      // 选区须落在文本容器内（排除工具栏按钮等）
      if (el.contains(range.commonAncestorContainer)) {
        const start = codePointOffsetBefore(el, range);
        const end = codePointOffsetTo(el, range.endContainer, range.endOffset);
        if (end > start) {
          setCaretPos(null); // 隐藏闪烁光标，交浏览器原生高亮
          invoke("set_selection", { start, end, text: displayedRef.current });
          return;
        }
      }
    }
    // 折叠（纯点击）→ 定位光标
    const range = (document as any).caretRangeFromPoint?.(e.clientX, e.clientY) as Range | undefined;
    if (!range) return;
    sel?.removeAllRanges();
    const offset = codePointOffsetBefore(el, range);
    setCaretPos(offset);
    invoke("set_caret", { offset });
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
    <div className="relative w-full h-full">
    <div
      id="result-container"
      className={cn(
        "absolute top-0 left-1/2 -translate-x-1/2 bg-background rounded-lg border border-black/[0.08] shadow-lg shadow-black/[0.06] flex flex-col overflow-hidden transition-all duration-200 ease-out",
        expanded ? "w-[720px] h-[480px]" : "w-[520px] h-[116px]",
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
        {/* tight relative wrapper：CaretBlink 浮层的定位基准（与 textRef 同原点、无 padding
            偏移，故 measureCaretPx 相对 textRef 量得的 px 直接可用）。
            textRef 的 contentEditable 子节点不让 React 跨编辑边界做 in-place reconcile：
            key 随 editing 切换 → view/edit 走 unmount/mount 而非在原地增删子节点，杜绝
            React 在用户浏览器 mutate 过的 contentEditable 上 removeChild 抛
            "The object can not be found here"。CaretBlink 移出 contentEditable 当兄弟浮层，
            避免与用户编辑的 DOM 抢同一个父节点的子节点位。 */}
        <div className="relative h-full">
          <div
            key={editing ? "edit" : "view"}
            ref={textRef}
            className={cn(
              "text-sm leading-[1.6] text-foreground overflow-y-auto",
              expanded ? "h-full" : "max-h-[63px]",
              "break-words outline-none thin-scrollbar",
              // whitespace-pre-wrap：编辑态 innerText 提取的 \n 在退出编辑（view 态）正确换行，
              // 否则 white-space:normal 把 \n 折叠成空格——编辑加的换行后端原样存（segment/DB 都不丢），仅显示丢。
              "whitespace-pre-wrap",
              !editing && "cursor-text",
            )}
            contentEditable={editing}
            suppressContentEditableWarning
            onInput={onTextInput}
            onMouseUp={!editing ? handleTextMouseUp : undefined}
            onScroll={(e) => {
              const el = e.currentTarget;
              const stick = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
              stickToBottomRef.current = stick;
              // 滚回底部区域（stick 恢复 true）→ 立即跟随到最新，不等下个 tick：tick 间隔 100-200ms，
              // 间隙内最新文字滞留视口下方"空白"，下个 tick 才 rAF 滚底 → 视觉上"连同最新一起出现"。
              if (stick) el.scrollTop = el.scrollHeight;
            }}
          >
            {text}
          </div>
          {!editing && <CaretBlink containerRef={textRef} text={text} pos={caretPos} />}
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

// ASR 光标 helpers（codePointOffsetTo/Before、measureCaretPx）已抽到 ./caret.ts。

// 闪烁光标：绝对定位到 pos 处的像素位置（相对文本容器）。
// 依赖 text/pos 变化重新量像素；container 经 textRef.current 透传。
// containerRef 而非 container：editing 切换致 textRef 重挂载（key edit→view）时，render 阶段
// 求值的 textRef.current 是**即将卸载的旧 div**（ref 在 commit 后才更新），传其 .current 会测到
// detached 旧 div → getBoundingClientRect 返回 (0,0) → 光标错落首位且不再重测。改传 RefObject，
// effect（commit 后执行）内读 .current 拿到已挂载的新 view div，量到真实末尾。
function CaretBlink({
  containerRef,
  text,
  pos,
}: {
  containerRef: React.RefObject<HTMLDivElement | null>;
  text: string;
  pos: number | null;
}) {
  const [px, setPx] = useState<{ left: number; top: number; height: number } | null>(null);
  // text/pos 变（流式追加 / 中插）量像素；用户滚动也须重测——px.top = rect.top-cRect.top 是视口相对
  // 值（随 scrollTop 变），不重测会停在旧位（流式末尾在容器底，用户上滚后末尾滚到视口下方，但光标仍
  // 停在容器底闪烁误导）。rAF 节流滚动高频，合并每帧一次 getBoundingClientRect。
  useEffect(() => {
    const el = containerRef.current;
    const measure = () => setPx(measureCaretPx(el, pos));
    // 初始测量推到下一帧：renderResultNow 同帧已 flushSync 写 textContent + 同步 re-render（DOM 写），
    // 若同帧立即 measure() 会同步 getBoundingClientRect → force reflow（layout thrashing；高频 ASR
    // 10-20Hz 下每帧叠加）。rAF 让 DOM 写先落地、布局稳定后再读——代价仅 1 帧（~16ms）光标滞后，肉眼
    // 无感，且天然合并到帧边界。滚动同样走 rAF 节流（见下）。
    let raf = requestAnimationFrame(measure);
    if (!el) {
      cancelAnimationFrame(raf);
      return;
    }
    let scrollRaf = 0;
    const onScroll = () => {
      if (scrollRaf) return;
      scrollRaf = requestAnimationFrame(() => { scrollRaf = 0; measure(); });
    };
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      cancelAnimationFrame(raf);
      el.removeEventListener("scroll", onScroll);
      if (scrollRaf) cancelAnimationFrame(scrollRaf);
    };
  }, [containerRef, text, pos]);
  if (!px) return null;
  // 末尾滚出容器可见区（用户上滚看历史）→ 隐藏光标。stickToBottom 时 px.top≈clientH-行高 < clientH
  // （显示）；上滚后末尾滚到视口下方 px.top>clientH → 隐藏。
  const el = containerRef.current;
  if (el && (px.top < -2 || px.top > el.clientHeight + 2)) return null;
  return (
    <span
      className="asr-caret"
      style={{ left: px.left, top: px.top, height: px.height }}
    />
  );
}
