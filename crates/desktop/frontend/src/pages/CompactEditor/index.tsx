import { useState, useRef, useEffect, useCallback } from "react";
import { invoke, listen } from "@/lib/tauri";
import {
  X, Type, Eye, Mic, FileText,
} from "lucide-react";
import ImagePreviewComponent from "@/pages/ImagePreview";
import { MarkdownPane } from "./MarkdownPane";
import { TranslationContrastPane } from "./TranslationContrastPane";
import { mergePendingTabs } from "./mergePendingTabs";
import { promoteTempTab } from "./promoteTempTab";
import TabHoverCard from "./TabHoverCard";
import { useT, t as ti18n } from "@/lib/i18n";

export interface Tab {
  key: string;
  source: 'clipboard' | 'transcription' | 'temp' | 'file';
  itemId: number;
  itemType?: 'text' | 'image';
  text?: string;
  imgWidth?: number;
  imgHeight?: number;
  isTemp?: boolean;
  mode?: 'single' | 'contrast';
  originalText?: string;
  translatedText?: string;
  // 流式翻译 sessionId（contrast tab 用，详见 open-tab handler）。
  translateSessionId?: string;
  // file source tab 的磁盘路径（保存写回用）
  filePath?: string;
}
interface OpenTabPayload {
  itemId: number;
  source: string;
  text?: string;
  isTemp?: boolean;
  mode?: string;
  originalText?: string;
  translatedText?: string;
  // 流式翻译 sessionId——后端 contrast tab 携带，前端据此建立 sessionId → tabKey 映射。
  // 2026-07-17 修复发现 1（竞态）+ 8（并发错路由）。
  translateSessionId?: string;
  // file tab 源文件路径（与 PendingTabFull.filePath 对齐，e29524d6 遗漏字段补全）
  filePath?: string;
}
// 后端 get_pending_compact_tabs 返回（含完整数据，前端免再查 DB）。
interface PendingTabFull {
  itemId: number;
  source: string;
  itemType: string;
  text: string;
  imgWidth?: number;
  imgHeight?: number;
  isTemp?: boolean;
  mode?: string;
  originalText?: string;
  translatedText?: string;
  translateSessionId?: string;
  filePath?: string;
}
interface TranslateSessionPayload {
  sessionId: string;
  text: string;
}
function pendingToTab(p: PendingTabFull): Tab {
  const key = p.isTemp ? `temp:${Date.now()}_${p.itemId}_${Math.random().toString(36).slice(2, 6)}` : `${p.source}:${p.itemId}`;
  const source = p.source as Tab['source'];
  if (p.itemType === 'image') {
    return { key, source, itemId: p.itemId, itemType: 'image', imgWidth: p.imgWidth || 0, imgHeight: p.imgHeight || 0 };
  }
  // file tab 的 originalText = 加载时的磁盘内容（外部变化检测用）
  const originalText = p.source === 'file' ? p.text : p.originalText;
  return { key, source, itemId: p.itemId, itemType: 'text', text: p.text, isTemp: p.isTemp, mode: p.mode as Tab['mode'], originalText, translatedText: p.translatedText, translateSessionId: p.translateSessionId, filePath: p.filePath };
}

const FONT_KEY = "compact-editor-font-size";
const FONT_MIN = 12;
const FONT_MAX = 24;
const MAX_IMAGE_TABS = 5;

function tabTitle(tab: Tab): string {
  const text = tab.text || "";
  const head = text.slice(0, 5).replace(/\s+/g, " ").trim() || (tab.itemType === 'image' ? ti18n("tab.image") : ti18n("tab.empty"));
  const tail = tab.itemId.toString(16).slice(-5);
  return `${head}-${tail}`;
}

function tabIcon(tab: Tab) {
  if (tab.source === 'transcription') return <Mic className="w-3 h-3 text-violet-500 flex-shrink-0" />;
  if (tab.source === 'file') return <FileText className="w-3 h-3 text-emerald-500 flex-shrink-0" />;
  if (tab.itemType === 'image') return <Eye className="w-3 h-3 text-blue-500 flex-shrink-0" />;
  return <Type className="w-3 h-3 text-muted-foreground flex-shrink-0" />;
}

// URL 参数初始化：Rust 建窗时拼入首个 tab 元数据（不含 text——避免超长 URL 白屏），
// 前端首次渲染即有 tab 占位（零 IPC），text 由 mount 后 get_pending_compact_tabs 补全。
function readInitialTabFromUrl(): { tabs: Tab[]; hasInitial: boolean } {
  const params = new URLSearchParams(window.location.search);
  const itemId = params.get("itemId");
  const source = params.get("source");
  if (!itemId || !source) return { tabs: [], hasInitial: false };
  const id = Number(itemId);
  const itemType = params.get("itemType") || "text";
  const key = `${source}:${id}`;
  if (itemType === "image") {
    const imgWidth = Number(params.get("imgWidth") || 0);
    const imgHeight = Number(params.get("imgHeight") || 0);
    return { tabs: [{ key, source: source as any, itemId: id, itemType: "image" as const, imgWidth, imgHeight }], hasInitial: true };
  }
  return { tabs: [{ key, source: source as any, itemId: id, itemType: "text" as const, text: "" }], hasInitial: true };
}

function CompactEditor() {
  const t = useT();
  const [initial] = useState(() => readInitialTabFromUrl());
  const [tabs, setTabs] = useState<Tab[]>(initial.tabs);
  const [initialLoading, setInitialLoading] = useState(!initial.hasInitial);
  const [activeIdx, setActiveIdx] = useState(0);
  const [fontSize, setFontSize] = useState(() => {
    const saved = Number(localStorage.getItem(FONT_KEY));
    return saved >= FONT_MIN && saved <= FONT_MAX ? saved : 15;
  });
  const [savedFlash, setSavedFlash] = useState(false);
  const [translating, setTranslating] = useState(false);
  const [hoveredTabKey, setHoveredTabKey] = useState<string | null>(null);
  const [hoveredTabRect, setHoveredTabRect] = useState<DOMRect | null>(null);
  const savedFlashTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const hoverTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => () => { if (savedFlashTimer.current) clearTimeout(savedFlashTimer.current); }, []);

  const tabsRef = useRef<Tab[]>([]);
  useEffect(() => { tabsRef.current = tabs; }, [tabs]);
  // 并发加载占位：loadAndAddTab 的 await 期间 tabsRef 尚未更新，快速连点同一 item 会
  // 两路都过 findIndex（-1）→ setTabs 各加一份 → 重复 key。await 前同步占位拦截。
  const pendingKeysRef = useRef<Set<string>>(new Set());

  const active = tabs[activeIdx];

  const updateActiveTextAt = useCallback((next: string, idx: number) => {
    setTabs(prev => prev.map((t, i) => (i === idx ? { ...t, text: next } : t)));
  }, []);

  // 更新 contrast 模式左栏（原文）。发现 4 修复——原先 onOriginalChange 错调
  // updateActiveTextAt 写幽灵字段 text（contrast 模式既不显示也不保存），导致用户
  // 编辑左栏原文后切 tab 编辑丢失（重建读 originalText 旧值）。
  const updateActiveOriginalAt = useCallback((next: string, idx: number) => {
    setTabs(prev => prev.map((t, i) => (i === idx ? { ...t, originalText: next } : t)));
  }, []);

  // 加载某 item 并新增 tab；已存在则切过去。source 决定从哪个表读 + 是否只读。
  // 基于 tabsRef.current 同步计算 next tabs（不依赖 setTabs updater 异步回调）。
  const loadAndAddTab = useCallback(async (itemId: number, source: string) => {
    const key = `${source}:${itemId}`;
    const existIdx = tabsRef.current.findIndex(t => t.key === key);
    if (existIdx >= 0) { setActiveIdx(existIdx); return; }
    if (pendingKeysRef.current.has(key)) return;
    pendingKeysRef.current.add(key);
    try {
      if (source === 'transcription') {
        const text = await invoke<string>("get_transcription_text", { id: itemId }).catch(() => "");
        if (tabsRef.current.some(t => t.key === key)) { setActiveIdx(tabsRef.current.findIndex(t => t.key === key)); return; }
        const next = [...tabsRef.current, { key, source: 'transcription' as const, itemId, text }];
        tabsRef.current = next;
        setTabs(next);
        setActiveIdx(next.length - 1);
        return;
      }

      // clipboard：先查类型，再加载
      const itemType = await invoke<string>("get_clipboard_item_type", { itemId }).catch(() => "text");
      if (itemType === 'image') {
        if (tabsRef.current.some(t => t.key === key)) { setActiveIdx(tabsRef.current.findIndex(t => t.key === key)); return; }
        // 图片 tab ≤5 限制
        const imageTabs = tabsRef.current.filter(t => t.itemType === 'image');
        let base = tabsRef.current;
        if (imageTabs.length >= MAX_IMAGE_TABS) {
          const oldestKey = imageTabs[0].key;
          base = tabsRef.current.filter(t => t.key !== oldestKey);
        }
        const next = [...base, { key, source: 'clipboard' as const, itemId, itemType: 'image' as const }];
        tabsRef.current = next;
        setTabs(next);
        setActiveIdx(next.length - 1);
      } else {
        const text = await invoke<string>("get_clipboard_item_text", { itemId }).catch(() => "");
        if (tabsRef.current.some(t => t.key === key)) { setActiveIdx(tabsRef.current.findIndex(t => t.key === key)); return; }
        const next = [...tabsRef.current, { key, source: 'clipboard' as const, itemId, itemType: 'text' as const, text }];
        tabsRef.current = next;
        setTabs(next);
        setActiveIdx(next.length - 1);
      }
    } finally {
      pendingKeysRef.current.delete(key);
    }
  }, []);

  // mount：先注册 listen（防止 take pending 后、listen 前的 emit 丢失），
  // 再 invoke get_pending_compact_tabs take 剩余 pending（与 URL 首个按 key 去重）。
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    let unlistenProgress: (() => void) | undefined;
    let unlistenDone: (() => void) | undefined;
    let unlistenFileChanged: (() => void) | undefined;
    (async () => {
      // 1. 先注册事件监听——确保 take pending 期间发出的 emit 不会丢失
      const fn = await listen("compact-editor://open-tab", (payload) => {
        const p = payload as OpenTabPayload;
        if (p.source === 'temp') {
          const tempKey = `temp:${Date.now()}_${Math.random().toString(36).slice(2, 6)}`;
          const newTab: Tab = { key: tempKey, source: 'temp' as const, itemId: 0, itemType: 'text' as const, text: p.text || "", isTemp: true, mode: (p.mode === 'contrast' ? 'contrast' : 'single'), originalText: p.originalText, translatedText: p.translatedText, translateSessionId: p.translateSessionId };
          // action bar 流式翻译开的 contrast tab——译文区 loading，
          // 用 sessionId 建立映射（发现 1+8 修复：不再依赖单值 ref 时序）
          if (newTab.mode === 'contrast' && (newTab.translatedText || "").startsWith("⏳") && newTab.translateSessionId) {
            registerTranslateSession(newTab.translateSessionId, tempKey);
          }
          const next = [...tabsRef.current, newTab];
          tabsRef.current = next;
          setTabs(next);
          setActiveIdx(next.length - 1);
        } else if (p.source === 'file') {
          // 文件查看：text 随事件携带（不查 DB），按 file:<itemId> 去重
          const fileKey = `file:${p.itemId}`;
          const existIdx = tabsRef.current.findIndex(t => t.key === fileKey);
          if (existIdx >= 0) { setActiveIdx(existIdx); return; } // 已存在→激活，不覆盖
          const newTab: Tab = { key: fileKey, source: 'file' as const, itemId: p.itemId, itemType: 'text' as const, text: p.text || "", originalText: p.text || "", filePath: p.filePath };
          const next = [...tabsRef.current, newTab];
          tabsRef.current = next;
          setTabs(next);
          setActiveIdx(next.length - 1);
        } else {
          loadAndAddTab(p.itemId, p.source);
        }
      });
      if (cancelled) { fn(); return; }
      unlisten = fn;

      // compact-editor://translate-progress：流式翻译逐段更新译文
      // 事件名与 Result 窗口的 translate-progress 彻底隔离（发现 6 修复）。
      // 按 payload.sessionId 路由到对应 tab（发现 1+8 修复）。
      const fnProgress = await listen("compact-editor://translate-progress", (payload) => {
        const p = payload as TranslateSessionPayload;
        const key = translatingSessionsRef.current.get(p.sessionId);
        if (!key) {
          // R2 兜底：tabKey 映射尚未建立（spawn emit 早于 open-tab 处理）→ 缓存等回放
          pendingTranslateEventsRef.current.set(p.sessionId, { text: p.text, done: false });
          return;
        }
        const next = tabsRef.current.map((t) =>
          t.key === key ? { ...t, translatedText: p.text } : t
        );
        tabsRef.current = next;
        setTabs(next);
      });
      if (cancelled) { fnProgress(); return; }
      unlistenProgress = fnProgress;

      // compact-editor://translate-done：翻译完成——从 Map 移除该 session（不清空整个 Map）
      const fnDone = await listen("compact-editor://translate-done", (payload) => {
        const p = payload as TranslateSessionPayload;
        const key = translatingSessionsRef.current.get(p.sessionId);
        translatingSessionsRef.current.delete(p.sessionId);
        if (translatingSessionsRef.current.size === 0) {
          setTranslating(false);
        }
        // 疑点 A 根治（2026-07-17）：listener 主路径（无论 key 是否存在）都通知后端
        // 丢弃缓存。done 已到达 → 后端缓存使命完成，立即释放。避免稳态常驻 64 条至 LRU 挤出。
        // fire-and-forget：失败不影响功能（后端 64 上限兜底）。
        invoke("forget_translate_result", { sessionId: p.sessionId }).catch(() => {});
        if (!key) {
          // R2 兜底：done 到达但映射未建立 → 缓存为 done 终止态，待 open-tab 写入 Map 时回放
          pendingTranslateEventsRef.current.set(p.sessionId, { text: p.text, done: true });
          return;
        }
        const next = tabsRef.current.map((t) =>
          t.key === key ? { ...t, translatedText: p.text } : t
        );
        tabsRef.current = next;
        setTabs(next);
      });
      if (cancelled) { fnDone(); return; }
      unlistenDone = fnDone;

      // file-changed：磁盘文件被外部修改 → 匹配 file tab → 无编辑自动 reload / 有编辑提示
      const fnFileChanged = await listen("compact-editor://file-changed", (payload) => {
        const changedPath = payload as string;
        const tab = tabsRef.current.find(t => t.source === 'file' && t.filePath === changedPath);
        if (!tab) return;
        // 用 originalText（加载时的磁盘内容）判断有无未保存编辑
        // tab.text 是 CM6 编辑后的当前值；originalText 是加载时的值
        const hasUnsavedEdits = (tab.text || "") !== (tab.originalText || "");
        if (hasUnsavedEdits) {
          // 有未保存编辑 → 不自动覆盖，打日志（后续可加 toast 提示）
          console.warn("[compact-editor] 文件被外部修改，但 tab 有未保存编辑，不自动 reload:", changedPath);
        } else {
          // 无编辑 → 静默 reload
          invoke<string>("read_file_text", { path: changedPath }).then((newText) => {
            const next = tabsRef.current.map(t =>
              t.key === tab.key ? { ...t, text: newText, originalText: newText } : t
            );
            tabsRef.current = next;
            setTabs(next);
          }).catch(() => {});
        }
      });
      if (cancelled) { fnFileChanged(); return; }
      unlistenFileChanged = fnFileChanged;

      // 2. 再 take pending tabs（此时 listen 已就绪）
      const pendingTabs = await invoke<PendingTabFull[]>("get_pending_compact_tabs");
      if (cancelled) return;
      if (pendingTabs.length > 0) {
        // pending 含完整数据（text/img 尺寸）；URL 占位 tab 同 key 但缺 text，
        // 须用 pending 覆盖占位——旧 `continue` 跳过 pending → 首个文本 tab 永远 text=""。
        // 图片 tab 按 itemId 加载不受影响，文本必须靠此补全。详见 mergePendingTabs 单测。
        const mapped = pendingTabs.map(pendingToTab);
        // action bar 流式翻译开的 pending contrast tab——译文区 loading，
        // 用 sessionId 建立映射（发现 1+8 修复 + R2 兜底回放）
        const loadingContrast = mapped.find(t => t.mode === 'contrast' && (t.translatedText || "").startsWith("⏳"));
        if (loadingContrast && loadingContrast.translateSessionId) {
          registerTranslateSession(loadingContrast.translateSessionId, loadingContrast.key);
        }
        const next = mergePendingTabs(tabsRef.current, mapped);
        tabsRef.current = next;
        setTabs(next);
        setActiveIdx(next.length - 1);
      }
      setInitialLoading(false);
    })();

    return () => {
      cancelled = true;
      unlisten?.();
      unlistenProgress?.();
      unlistenDone?.();
      unlistenFileChanged?.();
    };
  }, [loadAndAddTab, updateActiveTextAt]);

  const doSave = useCallback(async () => {
    if (!active) return;
    if (active.source === 'transcription') return;
    if (active.itemType && active.itemType !== 'text') return;
    // contrast 模式保存译文（右半），原文是脚手架不持久化
    const saveText = active.mode === 'contrast' ? (active.translatedText || "") : (active.text || "");
    try {
      // file tab：写回磁盘文件
      if (active.source === 'file' && active.filePath) {
        await invoke("save_file", { path: active.filePath, content: saveText });
        // 同步 originalText（保存后 = 磁盘内容，外部变化检测基准重置）
        const next = tabsRef.current.map(t => t.key === active.key ? { ...t, originalText: saveText } : t);
        tabsRef.current = next;
        setTabs(next);
        setSavedFlash(true);
        if (savedFlashTimer.current) clearTimeout(savedFlashTimer.current);
        savedFlashTimer.current = setTimeout(() => setSavedFlash(false), 1200);
        return;
      }
      // temp tab（图文编辑空白入口）：空→关闭 tab；非空→insert 新条目并升级为正式 clipboard tab。
      // 升级后 key/itemId/isTemp 同步（promoteTempTab），后续编辑走下方「既有条目 update」路径。
      if (active.isTemp) {
        if (saveText.trim() === "") {
          if (tabs.length <= 1) { invoke("close_compact_editor"); return; }
          const idx = activeIdx;
          const next = tabsRef.current.filter((_, i) => i !== idx);
          tabsRef.current = next;
          setTabs(next);
          setActiveIdx(Math.min(activeIdx, next.length - 1));
          return;
        }
        // contrast temp 升级前把 text 设为译文（promoteTempTab 依赖 text 作为条目内容）
        const tabsWithText = active.mode === 'contrast'
          ? tabsRef.current.map((t, i) => i === activeIdx ? { ...t, text: saveText } : t)
          : tabsRef.current;
        const newId = await invoke<number>("insert_clipboard_text_item", { text: saveText });
        const next = promoteTempTab(tabsWithText, activeIdx, newId);
        tabsRef.current = next;
        setTabs(next);
        setSavedFlash(true);
        if (savedFlashTimer.current) clearTimeout(savedFlashTimer.current);
        savedFlashTimer.current = setTimeout(() => setSavedFlash(false), 1200);
        return;
      }
      // 既有条目：空→删条目并关 tab；非空→update content。
      if (saveText.trim() === "") {
        await invoke("delete_clipboard_item", { id: active.itemId });
        if (tabs.length <= 1) {
          invoke("close_compact_editor");
          return;
        }
        const idx = activeIdx;
        const next = tabsRef.current.filter((_, i) => i !== idx);
        tabsRef.current = next;
        setTabs(next);
        setActiveIdx(idx === activeIdx ? Math.min(activeIdx, next.length - 1) : activeIdx > idx ? activeIdx - 1 : activeIdx);
        return;
      }
      await invoke("set_clipboard_item_text", { itemId: active.itemId, text: saveText });
      setSavedFlash(true);
      if (savedFlashTimer.current) clearTimeout(savedFlashTimer.current);
      savedFlashTimer.current = setTimeout(() => setSavedFlash(false), 1200);
    } catch (e) {
      console.error("保存失败:", e);
    }
  }, [active, activeIdx, tabs.length]);

  // keydown 监听器稳定化：doSave 依赖 [active, activeIdx, tabs.length]，active.text 每键变 → doSave
  // 每键拿新引用；若 keydown useEffect deps 含 doSave，监听器每键 remove+add（GC 压力）。改用 ref：
  const doSaveRef = useRef(doSave);
  useEffect(() => { doSaveRef.current = doSave; }, [doSave]);

  // 正在翻译的 session：sessionId → tabKey 映射。
  //
  // 2026-07-17 修复发现 1（竞态）+ 8（并发错路由）——原为单值 ref：
  // - 竞态：spawn emit translate-progress 早于主线程 open-tab emit，ref 仍为 null → 丢弃
  // - 并发：toolbar 同时翻译两个 tab，后设的 ref 覆盖前者 → 前者事件错路由到后者
  // 改为 Map 后按 sessionId 路由，每个翻译 session 独立。toolbar / ActionBar / Quick Execute
  // 任一路径触发的翻译都通过 translate_text 命令返回 sessionId，前端把 sessionId → tabKey 写入此 Map。
  const translatingSessionsRef = useRef<Map<string, string>>(new Map());
  // R2 修复（2026-07-17）：兜底新窗口路径下的 spawn-emit-早于-mount 竞态。
  // 后端 execute_action_bar_inner Local 分支 spawn translate 线程与 open_temp_compact_editor
  // 投递并行，新窗口路径下 React mount + 串行 await listen 耗时数百 ms，spawn 线程
  // 缓存命中可 < 100ms emit done → done 在 sessionId 写入 Map 前到达 → get 返回 undefined
  // → 永久 loading。缓存未知 sessionId 的最新 progress + done，sessionId 写入 Map 时回放。
  // 仅缓存最近一次 progress（中间增量可丢，回放最终态即可）+ 一次 done（终止信号）。
  const pendingTranslateEventsRef = useRef<Map<string, { text: string; done: boolean }>>(new Map());

  // 把 sessionId → tabKey 写入 Map，并回放该 session 缓存的事件（R2 兜底）。
  // 三处调用：open-tab handler / pending-take / handleTranslateForTab。
  const registerTranslateSession = useCallback((sessionId: string, tabKey: string) => {
    translatingSessionsRef.current.set(sessionId, tabKey);
    setTranslating(true);
    // 回放缓存——若 spawn emit 早于本调用到达，progress/done 已存在 pendingTranslateEventsRef
    const cached = pendingTranslateEventsRef.current.get(sessionId);
    if (cached) {
      pendingTranslateEventsRef.current.delete(sessionId);
      const next = tabsRef.current.map((t) =>
        t.key === tabKey ? { ...t, translatedText: cached.text } : t
      );
      tabsRef.current = next;
      setTabs(next);
      if (cached.done) {
        // 缓存的是 done 终止态——同步清理 session 状态
        translatingSessionsRef.current.delete(sessionId);
        if (translatingSessionsRef.current.size === 0) {
          setTranslating(false);
        }
      }
      return; // 已有缓存命中，无需再查后端
    }
    // R2 残余疑点根治：listener 未注册阶段（webview 加载中）Tauri fire-and-forget 丢弃了
    // done 事件 → pendingTranslateEventsRef 无从记录。主动 invoke 后端 done 缓存兜底——
    // - 返回 Some → session 已 done，直接显示终止态译文 + 清理 session
    // - 返回 None → session 未开始 / 进行中 / done 已被取走，等 listener（已注册必接管）
    //
    // 后端只缓存 done 终止态（不缓存 progress）——多段翻译时 progress 增量交给 listener
    // 实时更新，避免 invoke 旧快照覆盖 listener 更新的译文（瑕疵 1）。
    invoke<{ text: string } | null>("get_translate_result", { sessionId }).then((r) => {
      if (!r) return;
      // session 可能已被 done handler 清理——二次检查
      if (!translatingSessionsRef.current.has(sessionId)) return;
      const next = tabsRef.current.map((t) =>
        t.key === tabKey ? { ...t, translatedText: r.text } : t
      );
      tabsRef.current = next;
      setTabs(next);
      translatingSessionsRef.current.delete(sessionId);
      if (translatingSessionsRef.current.size === 0) {
        setTranslating(false);
      }
    }).catch(() => { /* 后端查询失败不阻断——listener 仍可能收到事件 */ });
  }, []);

  // 工具栏翻译按钮：fire-and-forget——立即切 contrast（译文 loading），后台翻译 emit 更新
  const handleTranslateForTab = useCallback((idx: number) => {
    const tab = tabsRef.current[idx];
    if (!tab || tab.source === 'transcription') return;
    // 发现 5 修复——contrast 模式下 tab.text 是后端脚手架（Local 为
    // "【翻译】\n⏳ 正在翻译…"，LLM 为 "【翻译】\n{旧译文}"），直接当源文本会翻译
    // 占位符，且 originalText: sourceText 会把原文永久覆盖成占位符。
    // contrast 模式必须读 originalText（真原文），plain 模式读 text。
    const sourceText = tab.mode === 'contrast' ? (tab.originalText || "") : (tab.text || "");
    if (!sourceText.trim()) return;

    setTranslating(true);
    const next = tabsRef.current.map((t, i) =>
      i === idx
        ? { ...t, mode: 'contrast' as const, originalText: sourceText, translatedText: '⏳ ' + ti18n("editor.translating") }
        : t
    );
    tabsRef.current = next;
    setTabs(next);

    // fire-and-forget——后端 spawn 线程，结果通过 emit 事件返回。
    // targetType: "compact_editor" → 走新事件名 compact-editor://translate-progress|done
    // 且后端生成 sessionId 返回，前端据 sessionId 路由到该 tab。
    invoke<string>("translate_text", { text: sourceText, targetType: "compact_editor" }).then((sessionId) => {
      if (sessionId) {
        // 注意：invoke 返回时 spawn 线程已开始，done 可能已到（被 handler 缓存到 pendingTranslateEventsRef）。
        // registerTranslateSession 会回放缓存，保证 tabKey 建立后立即同步译文。
        registerTranslateSession(sessionId, tab.key);
      }
    }).catch((e) => {
      console.error("翻译启动失败:", e);
      setTranslating(false);
      alert(ti18n("editor.translateFail") + ": " + String(e));
    });
  }, []);

  // 关闭 tab：仅剩一个则关窗；否则移除并修正 activeIdx。
  const closeTab = (idx: number) => {
    if (tabs.length <= 1) {
      invoke("close_compact_editor");
      return;
    }
    const next = tabsRef.current.filter((_, i) => i !== idx);
    tabsRef.current = next;
    setTabs(next);
    setActiveIdx(idx < activeIdx ? activeIdx - 1 : idx === activeIdx ? Math.min(activeIdx, next.length - 1) : activeIdx);
  };

  // 字号记忆
  useEffect(() => { localStorage.setItem(FONT_KEY, String(fontSize)); }, [fontSize]);

  // ── 快捷键（仅保留 Cmd+S / Cmd+Enter 保存）──
  // 判断全收敛进 doSave（单一事实源），keydown 无条件调 doSaveRef.current()。
  // 避免 keydown effect deps [active?.itemType] 在同类型 tab 间切换时陈旧闭包。
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.isComposing || e.keyCode === 229) return;
      const mod = e.metaKey || e.ctrlKey;
      if (mod && e.key === "Enter") { e.preventDefault(); doSaveRef.current(); return; }
      if (mod && e.key.toLowerCase() === "s") { e.preventDefault(); doSaveRef.current(); return; }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, []);

  return (
    <div className="flex flex-col h-full bg-background">
      {/* tab 栏 */}
      {tabs.length > 0 && (
        <div className="flex-shrink-0 flex items-center gap-0.5 px-1.5 py-1 border-b border-border bg-muted overflow-x-auto thin-scrollbar">
          {tabs.map((tab, i) => (
            <div
              key={tab.key}
              className={`group/tab relative flex items-center gap-1 pl-2.5 pr-1 py-1 rounded-md text-xs whitespace-nowrap cursor-pointer transition-colors ${
                i === activeIdx
                  ? "bg-background text-foreground shadow-sm border border-border"
                  : "text-muted-foreground hover:bg-accent"
              }`}
              onClick={() => setActiveIdx(i)}
              onMouseEnter={(e) => {
                if (hoverTimer.current) clearTimeout(hoverTimer.current);
                const rect = e.currentTarget.getBoundingClientRect();
                hoverTimer.current = setTimeout(() => {
                  setHoveredTabKey(tab.key);
                  setHoveredTabRect(rect);
                }, 500);
              }}
              onMouseLeave={() => {
                if (hoverTimer.current) clearTimeout(hoverTimer.current);
                setHoveredTabKey(null);
              }}
            >
              {tabIcon(tab)}
              <span className="max-w-[140px] truncate">{tabTitle(tab)}</span>
              <button
                type="button"
                title={t("tab.close")}
                onClick={(e) => { e.stopPropagation(); closeTab(i); }}
                className="p-0.5 rounded hover:bg-accent text-muted-foreground hover:text-foreground"
              >
                <X className="w-3 h-3" />
              </button>
              {hoveredTabKey === tab.key && hoveredTabRect && (
                <TabHoverCard tab={tab} rect={hoveredTabRect} />
              )}
            </div>
          ))}
        </div>
      )}

      {/* 内容区：所有 tab hidden 挂载（图片保持状态），仅活跃 tab 可见 */}
      {tabs.length > 0 ? (
        tabs.map((tab, i) => (
          <div key={tab.key} className="flex-1 flex flex-col min-h-0" style={{ display: i === activeIdx ? 'flex' : 'none' }}>
            {tab.itemType === 'image' ? (
              // 图片 Tab 懒加载：仅活跃 Tab 挂载 ImagePreview
              i === activeIdx ? (
                <ImagePreviewComponent imageId={tab.itemId} initialWidth={tab.imgWidth} initialHeight={tab.imgHeight} />
              ) : (
                <div className="flex-1 flex items-center justify-center text-xs text-muted-foreground bg-background">
                  {t("editor.imageTabHint")}
                </div>
              )
            ) : (
              // 文本/语音 tab：仅活跃 tab 挂载，contrast 渲染 TranslationContrastPane
              i === activeIdx ? (
                tab.mode === 'contrast' ? (
                  <TranslationContrastPane
                    originalText={tab.originalText || ''}
                    translatedText={tab.translatedText || ''}
                    readOnly={tab.source === 'transcription'}
                    fontSize={fontSize}
                    onFontSizeChange={setFontSize}
                    onOriginalChange={(next) => updateActiveOriginalAt(next, i)}
                    onTranslatedChange={(next) => setTabs(prev => prev.map((t, j) => j === i ? { ...t, translatedText: next } : t))}
                    onTranslate={() => handleTranslateForTab(i)}
                    onSave={doSave}
                    disableSave={tab.source === 'transcription'}
                    savedFlash={savedFlash}
                    translating={translating}
                  />
                ) : (
                  <MarkdownPane
                    text={tab.text || ''}
                    readOnly={tab.source === 'transcription'}
                    fontSize={fontSize}
                    onFontSizeChange={setFontSize}
                    onChange={(next) => updateActiveTextAt(next, i)}
                    onClear={() => updateActiveTextAt('', i)}
                    onSave={doSave}
                    disableSave={tab.source === 'transcription'}
                    savedFlash={savedFlash}
                    onTranslate={tab.source === 'transcription' ? undefined : () => handleTranslateForTab(i)}
                    translating={translating}
                  />
                )
              ) : (
                <div className="flex-1 flex items-center justify-center text-xs text-muted-foreground">
                  {t("editor.switchHint")}
                </div>
              )
            )}
          </div>
        ))
      ) : initialLoading ? (
        <div className="flex-1" />
      ) : (
        <div className="flex-1 flex items-center justify-center text-sm text-muted-foreground">{t("editor.noTabs")}</div>
      )}
    </div>
  );
}

export default CompactEditor;
