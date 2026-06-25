import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface HistoryRecord {
  id: number;
  created_at: string;
  engine: string;
  raw_text: string;
  polished_text: string | null;
  polish_status: string;
  duration_ms: number;
}

interface HistoryPanelProps {
  showToast: (msg: string) => void;
}

export default function HistoryPanel({ showToast }: HistoryPanelProps) {
  const [records, setRecords] = useState<HistoryRecord[]>([]);
  const [loading, setLoading] = useState(false);
  const [offset, setOffset] = useState(0);
  const [done, setDone] = useState(false);
  const [selectedIds, setSelectedIds] = useState<Set<number>>(new Set());

  const loadHistory = useCallback(async (resetOffset?: boolean) => {
    if (loading) return;
    setLoading(true);
    const o = resetOffset ? 0 : offset;
    try {
      const recs = await invoke<HistoryRecord[]>("get_history", { limit: 20, offset: o });
      if (resetOffset) {
        setRecords(recs);
        setSelectedIds(new Set());
      } else {
        setRecords((prev) => [...prev, ...recs]);
      }
      setOffset(o + recs.length);
      setDone(recs.length < 20);
    } catch (e) {
      showToast("加载历史失败：" + e);
    }
    setLoading(false);
  }, [loading, offset, showToast]);

  useEffect(() => { loadHistory(true); }, []);

  const toggleSelect = (id: number) => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      next.has(id) ? next.delete(id) : next.add(id);
      return next;
    });
  };

  const toggleSelectAll = (checked: boolean) => {
    if (checked) {
      setSelectedIds(new Set(records.map((r) => r.id)));
    } else {
      setSelectedIds(new Set());
    }
  };

  const deleteSelected = async () => {
    if (selectedIds.size === 0) return;
    try {
      const ids = Array.from(selectedIds);
      await invoke("delete_history", { ids });
      showToast(`已删除 ${ids.length} 条`);
      setSelectedIds(new Set());
      loadHistory(true);
    } catch (e) {
      showToast("删除失败：" + e);
    }
  };

  const copyRecord = async (id: number) => {
    const rec = records.find((r) => r.id === id);
    if (!rec) return;
    const text = rec.polished_text || rec.raw_text;
    try {
      await navigator.clipboard.writeText(text);
      showToast("已复制");
    } catch (e) {
      showToast("复制失败：" + e);
    }
  };

  const allChecked = records.length > 0 && selectedIds.size === records.length;
  const indeterminate = selectedIds.size > 0 && selectedIds.size < records.length;

  return (
    <div>
      {records.length > 0 && (
        <div className="flex items-center gap-4 py-2 pb-4 border-b border-border mb-2">
          <label className="flex items-center gap-1.5 text-sm cursor-pointer">
            <input
              type="checkbox"
              className="w-[18px] h-[18px]"
              checked={allChecked}
              ref={(el) => { if (el) el.indeterminate = indeterminate; }}
              onChange={(e) => toggleSelectAll(e.target.checked)}
            />
            <span>全选</span>
          </label>
          {selectedIds.size > 0 && (
            <span className="text-xs text-muted-foreground">已选 {selectedIds.size} 项</span>
          )}
          <div className="ml-auto">
            <button
              className="px-3.5 py-1.5 border border-red-500 rounded-md text-red-500 text-sm transition-colors hover:bg-red-50 disabled:opacity-50 disabled:cursor-not-allowed"
              disabled={selectedIds.size === 0}
              onClick={deleteSelected}
            >
              删除
            </button>
          </div>
        </div>
      )}

      {records.map((rec) => {
        const hasPolished = !!rec.polished_text;
        const primaryText = hasPolished ? rec.polished_text! : rec.raw_text;
        const secondaryText = hasPolished ? rec.raw_text : null;
        const statusText: Record<string, string> = { done: "已润色", failed: "润色失败", off: "未润色" };
        const duration = rec.duration_ms ? (rec.duration_ms / 1000).toFixed(1) + "s" : "";

        return (
          <div key={rec.id} className="py-3 border-b border-border flex gap-2.5 items-start">
            <input
              type="checkbox"
              className="mt-0.5 flex-shrink-0 w-[18px] h-[18px] cursor-pointer"
              checked={selectedIds.has(rec.id)}
              onChange={() => toggleSelect(rec.id)}
            />
            <div className="flex-1 min-w-0">
              <div className="text-xs text-muted-foreground mb-1">{rec.created_at}</div>
              <div className="text-sm leading-[1.5] break-words">{primaryText}</div>
              {secondaryText && (
                <details className="mt-1">
                  <summary className="text-primary cursor-pointer text-xs">展开原始</summary>
                  <div className="text-sm leading-[1.5] break-words text-muted-foreground mt-1">{secondaryText}</div>
                </details>
              )}
              <div className="text-[11px] text-muted-foreground mt-1 flex gap-3">
                <span>{rec.engine}</span>
                <span>{statusText[rec.polish_status] || rec.polish_status}</span>
                {duration && <span>{duration}</span>}
              </div>
            </div>
            <button
              className="flex-shrink-0 p-1 text-muted-foreground hover:text-primary transition-colors"
              title="拷贝"
              onClick={() => copyRecord(rec.id)}
            >
              <svg className="w-4 h-4 fill-current" viewBox="0 0 640 640"><path d="M288 64C252.7 64 224 92.7 224 128L224 384C224 419.3 252.7 448 288 448L480 448C515.3 448 544 419.3 544 384L544 183.4C544 166 536.9 149.3 524.3 137.2L466.6 81.8C454.7 70.4 438.8 64 422.3 64L288 64zM160 192C124.7 192 96 220.7 96 256L96 512C96 547.3 124.7 576 160 576L352 576C387.3 576 416 547.3 416 512L416 496L352 496L352 512L160 512L160 256L176 256L176 192L160 192z" /></svg>
            </button>
          </div>
        );
      })}

      {loading && <div className="text-center py-4 text-muted-foreground">加载中...</div>}
      {!loading && !done && records.length > 0 && (
        <button
          className="w-full py-3 text-sm text-primary hover:underline"
          onClick={() => loadHistory()}
        >
          加载更多
        </button>
      )}
      {!loading && records.length === 0 && (
        <div className="text-center py-12 text-muted-foreground">暂无识别记录</div>
      )}
    </div>
  );
}
