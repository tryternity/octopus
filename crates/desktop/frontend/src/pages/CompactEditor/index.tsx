import { useState, useRef, useEffect, useCallback } from "react";
import { invoke, listen } from "@/lib/tauri";
import {
  X, Type, Eye, Mic,
} from "lucide-react";
import ImagePreviewComponent from "@/pages/ImagePreview";
import { MarkdownPane } from "./MarkdownPane";
import { useT, t as ti18n } from "@/lib/i18n";

interface Tab {
  key: string;
  source: 'clipboard' | 'transcription' | 'temp';
  itemId: number;
  itemType?: 'text' | 'image';
  text?: string;
  imgWidth?: number;
  imgHeight?: number;
  isTemp?: boolean;
}
interface OpenTabPayload {
  itemId: number;
  source: string;
  text?: string;
  isTemp?: boolean;
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
}
function pendingToTab(p: PendingTabFull): Tab {
  const key = p.isTemp ? `temp:${Date.now()}_${p.itemId}_${Math.random().toString(36).slice(2, 6)}` : `${p.source}:${p.itemId}`;
  const source = p.source as Tab['source'];
  if (p.itemType === 'image') {
    return { key, source, itemId: p.itemId, itemType: 'image', imgWidth: p.imgWidth || 0, imgHeight: p.imgHeight || 0 };
  }
  return { key, source, itemId: p.itemId, itemType: 'text', text: p.text, isTemp: p.isTemp };
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
  if (tab.itemType === 'image') return <Eye className="w-3 h-3 text-blue-500 flex-shrink-0" />;
  return <Type className="w-3 h-3 text-muted-foreground flex-shrink-0" />;
}

// URL 参数初始化：Rust 建窗时拼入首个 tab 数据，前端首次渲染即有内容（零 IPC）。
function readInitialTabFromUrl(): { tabs: Tab[]; hasInitial: boolean } {
  const params = new URLSearchParams(window.location.search);
  const itemId = params.get("itemId");
  const source = params.get("source");
  if (!itemId || !source) return { tabs: [], hasInitial: false };
  const id = Number(itemId);
  const itemType = params.get("itemType") || "text";
  const text = params.get("text") || "";
  const key = `${source}:${id}`;
  if (itemType === "image") {
    const imgWidth = Number(params.get("imgWidth") || 0);
    const imgHeight = Number(params.get("imgHeight") || 0);
    return { tabs: [{ key, source: source as any, itemId: id, itemType: "image" as const, imgWidth, imgHeight }], hasInitial: true };
  }
  return { tabs: [{ key, source: source as any, itemId: id, itemType: "text" as const, text }], hasInitial: true };
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
  const savedFlashTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
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

  // 加载某 item 并新增 tab；已存在则切过去。source 决定从哪个表读 + 是否只读。
  const loadAndAddTab = useCallback(async (itemId: number, source: string) => {
    const key = `${source}:${itemId}`;
    const existIdx = tabsRef.current.findIndex(t => t.key === key);
    if (existIdx >= 0) { setActiveIdx(existIdx); return; }
    if (pendingKeysRef.current.has(key)) return;
    pendingKeysRef.current.add(key);
    try {
      if (source === 'transcription') {
        const text = await invoke<string>("get_transcription_text", { id: itemId }).catch(() => "");
        setTabs(prev => prev.some(t => t.key === key) ? prev : [...prev, { key, source: 'transcription', itemId, text }]);
        setTabs(prev => { const idx = prev.findIndex(t => t.key === key); if (idx >= 0) { tabsRef.current = prev; setActiveIdx(idx); } return prev; });
        return;
      }

      // clipboard：先查类型，再加载
      const itemType = await invoke<string>("get_clipboard_item_type", { itemId }).catch(() => "text");
      if (itemType === 'image') {
        // 图片 tab ≤5 限制
        setTabs(prev => {
          if (prev.some(t => t.key === key)) return prev;
          const imageTabs = prev.filter(t => t.itemType === 'image');
          let next = prev;
          if (imageTabs.length >= MAX_IMAGE_TABS) {
            const oldestKey = imageTabs[0].key;
            next = prev.filter(t => t.key !== oldestKey);
          }
          return [...next, { key, source: 'clipboard' as const, itemId, itemType: 'image' as const }];
        });
        setTabs(prev => {
          const newIdx = prev.findIndex(t => t.key === key);
          if (newIdx >= 0) {
            tabsRef.current = prev;
            setActiveIdx(newIdx);
          }
          return prev;
        });
      } else {
        const text = await invoke<string>("get_clipboard_item_text", { itemId }).catch(() => "");
        setTabs(prev => prev.some(t => t.key === key) ? prev : [...prev, { key, source: 'clipboard' as const, itemId, itemType: 'text' as const, text }]);
        setTabs(prev => { const idx = prev.findIndex(t => t.key === key); if (idx >= 0) { tabsRef.current = prev; setActiveIdx(idx); } return prev; });
      }
    } finally {
      pendingKeysRef.current.delete(key);
    }
  }, []);

  // mount：URL 已注入首个 tab（零 IPC）；invoke get_pending_compact_tabs take 剩余
  // pending（批量双开场景：URL 只注入首个，其余 pending tab 于此补齐，与 URL 首个
  // 按 key 去重）；注册 open-tab 事件用于后续已 mount 窗口的新开 tab。
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    (async () => {
      const pendingTabs = await invoke<PendingTabFull[]>("get_pending_compact_tabs");
      if (cancelled) return;
      if (pendingTabs.length > 0) {
        setTabs(prev => {
          const seen = new Set(prev.map(t => t.key));
          const added: Tab[] = [];
          for (const p of pendingTabs) {
            const tab = pendingToTab(p);
            if (seen.has(tab.key)) continue;
            seen.add(tab.key);
            added.push(tab);
          }
          return added.length > 0 ? [...prev, ...added] : prev;
        });
      }
      setInitialLoading(false);
      const fn = await listen("compact-editor://open-tab", (payload) => {
        const p = payload as OpenTabPayload;
        if (p.source === 'temp') {
          const tempKey = `temp:${Date.now()}`;
          setTabs(prev => [...prev, { key: tempKey, source: 'temp', itemId: 0, itemType: 'text', text: p.text, isTemp: true }]);
          setTabs(prev => { const idx = prev.findIndex(t => t.key === tempKey); if (idx >= 0) { tabsRef.current = prev; setActiveIdx(idx); } return prev; });
        } else {
          loadAndAddTab(p.itemId, p.source);
        }
      });
      if (cancelled) fn();
      else unlisten = fn;
    })();
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [loadAndAddTab]);

  const doSave = useCallback(async () => {
    if (!active) return;
    if (active.isTemp) return;
    if (active.source === 'transcription') return;
    if (active.itemType && active.itemType !== 'text') return;
    try {
      if ((active.text || "").trim() === "") {
        await invoke("delete_clipboard_item", { id: active.itemId });
        if (tabs.length <= 1) {
          invoke("close_compact_editor");
          return;
        }
        const idx = activeIdx;
        setTabs(prev => prev.filter((_, i) => i !== idx));
        setActiveIdx(i => (idx < i ? i - 1 : idx === i ? Math.min(i, tabs.length - 2) : i));
        return;
      }
      await invoke("set_clipboard_item_text", { itemId: active.itemId, text: active.text || "" });
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

  // 关闭 tab：仅剩一个则关窗；否则移除并修正 activeIdx。
  const closeTab = (idx: number) => {
    if (tabs.length <= 1) {
      invoke("close_compact_editor");
      return;
    }
    setTabs(prev => prev.filter((_, i) => i !== idx));
    setActiveIdx(i => {
      if (idx < i) return i - 1;
      if (idx === i) return Math.min(i, tabs.length - 2);
      return i;
    });
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
              className={`group/tab flex items-center gap-1 pl-2.5 pr-1 py-1 rounded-md text-xs whitespace-nowrap cursor-pointer transition-colors ${
                i === activeIdx
                  ? "bg-background text-foreground shadow-sm border border-border"
                  : "text-muted-foreground hover:bg-accent"
              }`}
              onClick={() => setActiveIdx(i)}
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
            </div>
          ))}
        </div>
      )}

      {/* 内容区：所有 tab hidden 挂载（图片保持状态），仅活跃 tab 可见 */}
      {tabs.length > 0 ? (
        tabs.map((tab, i) => (
          <div key={tab.key} className="flex-1 flex flex-col" style={{ display: i === activeIdx ? 'flex' : 'none' }}>
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
              // 文本/语音 tab：仅活跃 tab 挂载 MarkdownPane
              i === activeIdx ? (
                <MarkdownPane
                  text={tab.text || ''}
                  readOnly={tab.source === 'transcription'}
                  fontSize={fontSize}
                  onFontSizeChange={setFontSize}
                  onChange={(next) => updateActiveTextAt(next, i)}
                  onClear={() => updateActiveTextAt('', i)}
                  onSave={doSave}
                  disableSave={active?.isTemp || tab.source === 'transcription'}
                  savedFlash={savedFlash}
                />
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
