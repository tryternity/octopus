import { useCallback, useEffect, useState, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { cn } from "@/lib/utils";
import { useInfiniteScroll } from "@/hooks/useInfiniteScroll";
import { Copy, Trash2, ChevronDown, Search, ChevronRight } from "lucide-react";

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
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [search, setSearch] = useState("");
  const debouncedSearch = useDebouncedValue(search, 300);
  const pendingResetRef = useRef(false);

  const loadHistory = useCallback(async (resetOffset?: boolean) => {
    if (loading) {
      if (resetOffset) pendingResetRef.current = true;
      return;
    }
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
    // 如果加载期间收到 reset 请求，补执行
    if (pendingResetRef.current) {
      pendingResetRef.current = false;
      loadHistory(true);
    }
  }, [loading, offset, showToast, debouncedSearch]);

  useEffect(() => { loadHistory(true); }, [debouncedSearch]);

  const toggleSelect = (id: number) => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      next.has(id) ? next.delete(id) : next.add(id);
      return next;
    });
    setConfirmDelete(false);
  };

  const allChecked = records.length > 0 && selectedIds.size === records.length;
  const hasSelection = selectedIds.size > 0;

  const handleBatchDelete = async () => {
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

  const handleSingleDeleted = () => {
    setSelectedIds(new Set());
    loadHistory(true);
  };

  const sentinelRef = useInfiniteScroll(() => loadHistory(), loading, done);

  return (
    <div className="flex flex-col h-full">
      {/* ── 搜索（置顶）── */}
      <div className="pb-3 border-b border-border">
        <div className="flex items-center gap-2 px-2.5 py-1.5 bg-stone-50 rounded-md border border-stone-200">
          <Search className="w-3.5 h-3.5 text-stone-400" />
          <input
            type="text"
            placeholder="搜索识别文本..."
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            className="flex-1 bg-transparent text-xs outline-none placeholder:text-stone-400"
          />
        </div>
      </div>

      {/* ── 列表 ── */}
      <div className="flex-1 overflow-y-auto thin-scrollbar -mx-1 px-1">
        {/* 列表 header：全选 */}
        {records.length > 0 && (
          <div className="flex items-center gap-2 px-3 py-1.5 border-b border-stone-100 group/header">
            <label className="flex items-center gap-1.5 cursor-pointer">
              <input
                type="checkbox"
                className="w-3.5 h-3.5"
                checked={allChecked}
                onChange={(e) => setSelectedIds(e.target.checked ? new Set(records.map((r) => r.id)) : new Set())}
              />
              <span className="text-[10px] text-stone-400 group-hover/header:text-stone-600 transition-colors">
                {hasSelection ? `已选 ${selectedIds.size} 项` : "全选"}
              </span>
            </label>
          </div>
        )}

        {records.length === 0 && !loading && (
          <div className="flex flex-col items-center justify-center py-16 gap-1 text-stone-400">
            <span className="text-sm">暂无识别记录</span>
          </div>
        )}

        {records.map((rec) => (
          <HistoryRow
            key={rec.id}
            rec={rec}
            isSelected={selectedIds.has(rec.id)}
            onToggleSelect={() => toggleSelect(rec.id)}
            showToast={showToast}
            onDeleted={handleSingleDeleted}
          />
        ))}

        {loading && (
          <div className="text-center py-4 text-stone-400 text-xs">加载中...</div>
        )}
        {!loading && done && records.length > 0 && (
          <div className="text-center py-3 text-stone-300 text-[10px]">— 没有更多了 —</div>
        )}
        {/* 无限滚动 sentinel */}
        <div ref={sentinelRef} className="h-1" />
      </div>

      {/* ── 底部：状态 + 批量操作 ── */}
      <div className="flex items-center justify-between py-2 border-t border-border">
        <span className="text-[10px] text-stone-400">
          共 {records.length} 条记录
        </span>
        {hasSelection ? (
          <button
            className={cn(
              "flex items-center gap-1 px-2.5 py-1 rounded-md text-xs font-medium transition-all duration-150",
              confirmDelete
                ? "bg-red-600 text-white"
                : "border border-red-400 text-red-500 hover:bg-red-50",
            )}
            onClick={handleBatchDelete}
          >
            <Trash2 className="w-3 h-3" />
            {confirmDelete ? `确认删除 ${selectedIds.size} 项` : "删除选中"}
          </button>
        ) : (
          <span className="text-[10px] text-stone-300">{done ? "已全部加载" : `已加载 ${records.length} 条`}</span>
        )}
      </div>
    </div>
  );
}

/// 单条识别记录行——含 checkbox + 文本 + 折叠原始 + hover 操作（复制/单条删除）
function HistoryRow({
  rec,
  isSelected,
  onToggleSelect,
  showToast,
  onDeleted,
}: {
  rec: HistoryRecord;
  isSelected: boolean;
  onToggleSelect: () => void;
  showToast: (msg: string) => void;
  onDeleted: () => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const [deletePending, setDeletePending] = useState(false);
  const deleteTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    return () => { if (deleteTimer.current) clearTimeout(deleteTimer.current); };
  }, []);

  const hasPolished = !!rec.polished_text;
  const primaryText = hasPolished ? rec.polished_text! : rec.raw_text;
  const secondaryText = hasPolished ? rec.raw_text : null;
  const isPolished = rec.polish_status === "done";
  const duration = rec.duration_ms ? (rec.duration_ms / 1000).toFixed(1) + "s" : null;

  const copyRecord = async (e: React.MouseEvent) => {
    e.stopPropagation();
    try {
      await navigator.clipboard.writeText(rec.polished_text || rec.raw_text);
      showToast("已复制");
    } catch (e) { showToast("复制失败：" + e); }
  };

  const handleDeleteClick = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (!deletePending) {
      setDeletePending(true);
      deleteTimer.current = setTimeout(() => setDeletePending(false), 1500);
    } else {
      if (deleteTimer.current) clearTimeout(deleteTimer.current);
      try {
        await invoke("delete_history", { ids: [rec.id] });
        showToast("已删除");
        onDeleted();
      } catch (e) { showToast("删除失败：" + e); }
    }
  };

  return (
    <div
      className={cn(
        "group relative flex items-start gap-2.5 px-3 py-2.5 border-b border-stone-100/60 transition-colors cursor-pointer",
        isSelected ? "bg-stone-100" : "hover:bg-stone-50",
        deletePending && "bg-red-50",
      )}
      onClick={onToggleSelect}
    >
      {isPolished && (
        <div className="absolute left-0 top-2 bottom-2 w-[2px] rounded-r bg-amber-600/40" />
      )}
      <input
        type="checkbox"
        className="w-3.5 h-3.5 mt-0.5 flex-shrink-0"
        checked={isSelected}
        onChange={(e) => { e.stopPropagation(); onToggleSelect(); }}
        onClick={(e) => e.stopPropagation()}
      />
      <div className="flex-1 min-w-0">
        {/* Meta row */}
        <div className="flex items-center gap-1.5 mb-0.5 flex-wrap">
          <span className="text-[10px] text-stone-400">{rec.created_at}</span>
          {duration && (
            <span className="text-[10px] text-stone-400 px-1 rounded bg-stone-100">{duration}</span>
          )}
          <span className={cn(
            "text-[10px] px-1.5 py-0.5 rounded font-medium",
            isPolished ? "bg-amber-600/10 text-amber-700" : "text-stone-400",
          )}>
            {POLISH_LABELS[rec.polish_status] || rec.polish_status}
          </span>
          <span className="text-[10px] text-stone-300">{rec.engine}</span>
        </div>
        {/* Primary text */}
        <p className="text-xs leading-relaxed text-stone-800 break-words">{primaryText}</p>
        {/* Secondary (raw) text — collapsible */}
        {secondaryText && (
          <div className="mt-1">
            {expanded ? (
              <div>
                <div className="text-[11px] leading-relaxed text-stone-400 break-words pl-2 border-l border-stone-200">
                  {secondaryText}
                </div>
                <button
                  className="flex items-center gap-0.5 text-[10px] text-stone-400 hover:text-stone-700 mt-0.5 transition-colors"
                  onClick={(e) => { e.stopPropagation(); setExpanded(false); }}
                >
                  <ChevronDown className="w-2.5 h-2.5" />
                  收起原始
                </button>
              </div>
            ) : (
              <button
                className="flex items-center gap-0.5 text-[10px] text-stone-400 hover:text-stone-700 transition-colors"
                onClick={(e) => { e.stopPropagation(); setExpanded(true); }}
              >
                <ChevronRight className="w-2.5 h-2.5" />
                展开原始
              </button>
            )}
          </div>
        )}
      </div>

      {/* 右侧操作：复制 + 删除 */}
      <div className="flex-shrink-0 flex items-center gap-0.5">
        <button
          className="p-1 rounded opacity-0 group-hover:opacity-50 hover:!opacity-100 transition-opacity"
          onClick={copyRecord}
          title="复制"
        >
          <Copy className="w-3.5 h-3.5 text-stone-500 hover:text-stone-800" />
        </button>
        <button
          className={cn(
            "p-1 rounded transition-all",
            deletePending
              ? "opacity-100 bg-red-100"
              : "opacity-0 group-hover:opacity-50 hover:!opacity-100 transition-opacity",
          )}
          onClick={handleDeleteClick}
          title={deletePending ? "再次点击确认删除" : "删除"}
        >
          <Trash2 className={cn(
            "w-3.5 h-3.5 transition-colors",
            deletePending ? "text-red-600" : "text-stone-500 hover:text-red-500",
          )} />
        </button>
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
