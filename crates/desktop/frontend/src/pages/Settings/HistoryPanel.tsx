import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { cn } from "@/lib/utils";
import { Copy, Trash2, ChevronDown, Search } from "lucide-react";

interface HistoryRecord {
  id: number;
  created_at: string;
  engine: string;
  raw_text: string;
  polished_text: string | null;
  polish_status: string;
  duration_ms: number;
}

const POLISH_LABELS: Record<string, string> = { done: "已润色", failed: "润色失败", off: "未润色" };

export default function HistoryPanel({ showToast }: { showToast: (msg: string) => void }) {
  const [records, setRecords] = useState<HistoryRecord[]>([]);
  const [loading, setLoading] = useState(false);
  const [offset, setOffset] = useState(0);
  const [done, setDone] = useState(false);
  const [selectedIds, setSelectedIds] = useState<Set<number>>(new Set());
  const [expandedIds, setExpandedIds] = useState<Set<number>>(new Set());
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [search, setSearch] = useState("");
  const debouncedSearch = useDebouncedValue(search, 300);

  const loadHistory = useCallback(async (resetOffset?: boolean) => {
    if (loading) return;
    setLoading(true);
    const o = resetOffset ? 0 : offset;
    try {
      const recs = await invoke<HistoryRecord[]>("get_history", {
        limit: 20, offset: o,
        search: debouncedSearch || null,
      });
      if (resetOffset) { setRecords(recs); setSelectedIds(new Set()); }
      else { setRecords((prev) => [...prev, ...recs]); }
      setOffset(o + recs.length);
      setDone(recs.length < 20);
    } catch (e) { showToast("加载历史失败：" + e); }
    setLoading(false);
  }, [loading, offset, showToast, debouncedSearch]);

  useEffect(() => { loadHistory(true); }, [debouncedSearch]);

  const toggleSelect = (id: number) => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      next.has(id) ? next.delete(id) : next.add(id);
      return next;
    });
  };

  const allChecked = records.length > 0 && selectedIds.size === records.length;
  const indeterminate = selectedIds.size > 0 && selectedIds.size < records.length;

  const handleDelete = async () => {
    if (selectedIds.size === 0) return;
    if (!confirmDelete) {
      setConfirmDelete(true);
      setTimeout(() => setConfirmDelete(false), 3000);
      return;
    }
    try {
      const ids = Array.from(selectedIds);
      await invoke("delete_history", { ids });
      showToast(`已删除 ${ids.length} 条`);
      setSelectedIds(new Set());
      setConfirmDelete(false);
      loadHistory(true);
    } catch (e) { showToast("删除失败：" + e); }
  };

  const copyRecord = async (id: number, e: React.MouseEvent) => {
    e.stopPropagation();
    const rec = records.find((r) => r.id === id);
    if (!rec) return;
    try {
      await navigator.clipboard.writeText(rec.polished_text || rec.raw_text);
      showToast("已复制");
    } catch (e) { showToast("复制失败：" + e); }
  };

  const toggleExpand = (id: number) => {
    setExpandedIds((prev) => {
      const next = new Set(prev);
      next.has(id) ? next.delete(id) : next.add(id);
      return next;
    });
  };

  return (
    <div className="flex flex-col h-full">
      {/* Toolbar */}
      <div className="flex items-center gap-3 pb-3 border-b border-border">
        <label className="flex items-center gap-1.5 text-sm cursor-pointer">
          <input
            type="checkbox"
            className="w-4 h-4"
            checked={allChecked}
            ref={(el) => { if (el) el.indeterminate = indeterminate; }}
            onChange={(e) => setSelectedIds(e.target.checked ? new Set(records.map((r) => r.id)) : new Set())}
          />
          <span className="text-muted-foreground">全选</span>
        </label>
        {selectedIds.size > 0 && (
          <span className="text-xs text-muted-foreground">已选 {selectedIds.size} 项</span>
        )}
        <div className="flex-1" />
        <button
          className={cn(
            "flex items-center gap-1 px-3 py-1 border rounded-md text-sm transition-colors",
            confirmDelete ? "border-red-600 text-white bg-red-600" : "border-red-500 text-red-500 hover:bg-red-50",
            selectedIds.size === 0 && "opacity-40 cursor-not-allowed",
          )}
          disabled={selectedIds.size === 0}
          onClick={handleDelete}
        >
          <Trash2 className="w-3.5 h-3.5" />
          {confirmDelete ? "确认删除" : "删除选中"}
        </button>
      </div>

      {/* Search */}
      <div className="flex items-center gap-2 py-3 px-3 bg-muted rounded-md border border-border">
        <Search className="w-4 h-4 text-muted-foreground" />
        <input
          type="text"
          placeholder="搜索识别文本..."
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          className="flex-1 bg-transparent text-sm outline-none placeholder:text-muted-foreground/60"
        />
      </div>

      {/* List */}
      <div className="flex-1 overflow-y-auto thin-scrollbar -mx-1 px-1">
        {records.map((rec) => {
          const hasPolished = !!rec.polished_text;
          const primaryText = hasPolished ? rec.polished_text! : rec.raw_text;
          const secondaryText = hasPolished ? rec.raw_text : null;
          const isExpanded = expandedIds.has(rec.id);
          const isSelected = selectedIds.has(rec.id);
          const isPolished = rec.polish_status === "done";
          const duration = rec.duration_ms ? (rec.duration_ms / 1000).toFixed(1) + "s" : null;

          return (
            <div
              key={rec.id}
              className={cn(
                "group flex items-start gap-2.5 px-3 py-3 border-b border-border/40 transition-colors",
                isSelected ? "bg-accent" : "hover:bg-accent/50",
              )}
              onClick={() => toggleSelect(rec.id)}
            >
              <input
                type="checkbox"
                className="w-4 h-4 mt-0.5 flex-shrink-0"
                checked={isSelected}
                onChange={(e) => { e.stopPropagation(); toggleSelect(rec.id); }}
                onClick={(e) => e.stopPropagation()}
              />
              <div className="flex-1 min-w-0">
                {/* Meta row */}
                <div className="flex items-center gap-2 mb-1">
                  <span className="text-[10px] text-muted-foreground/70">{rec.created_at}</span>
                  {duration && (
                    <span className="text-[10px] text-muted-foreground/50 px-1 rounded bg-muted">{duration}</span>
                  )}
                  <span className={cn(
                    "text-[10px] px-1.5 py-0.5 rounded font-medium",
                    isPolished ? "bg-voice/10 text-voice" : "text-muted-foreground/60",
                  )}>
                    {POLISH_LABELS[rec.polish_status] || rec.polish_status}
                  </span>
                  <span className="text-[10px] text-muted-foreground/50">{rec.engine}</span>
                </div>
                {/* Primary text */}
                <p className="text-sm leading-relaxed text-foreground/90 break-words">{primaryText}</p>
                {/* Secondary (raw) text — collapsible */}
                {secondaryText && (
                  <div className="mt-1">
                    {isExpanded ? (
                      <div>
                        <div className="text-xs leading-relaxed text-muted-foreground/70 break-words">{secondaryText}</div>
                        <button
                          className="text-[11px] text-muted-foreground hover:text-foreground mt-0.5"
                          onClick={(e) => { e.stopPropagation(); toggleExpand(rec.id); }}
                        >
                          收起原始
                        </button>
                      </div>
                    ) : (
                      <button
                        className="flex items-center gap-1 text-[11px] text-muted-foreground hover:text-foreground"
                        onClick={(e) => { e.stopPropagation(); toggleExpand(rec.id); }}
                      >
                        <ChevronDown className="w-3 h-3" />
                        展开原始
                      </button>
                    )}
                  </div>
                )}
              </div>
              <button
                className="p-1 text-muted-foreground hover:text-foreground transition-colors"
                title="复制"
                onClick={(e) => copyRecord(rec.id, e)}
              >
                <Copy className="w-4 h-4" />
              </button>
            </div>
          );
        })}

        {loading && <div className="text-center py-4 text-muted-foreground text-sm">加载中...</div>}
        {!loading && !done && records.length > 0 && (
          <button
            className="w-full py-3 text-sm text-primary hover:underline"
            onClick={() => loadHistory()}
          >
            加载更多
          </button>
        )}
        {!loading && records.length === 0 && (
          <div className="text-center py-12 text-muted-foreground text-sm">暂无识别记录</div>
        )}
      </div>
    </div>
  );
}

function useDebouncedValue<T>(value: T, delay: number): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const timer = setTimeout(() => setDebounced(value), delay);
    return () => clearTimeout(timer);
  }, [value, delay]);
  return debounced;
}
