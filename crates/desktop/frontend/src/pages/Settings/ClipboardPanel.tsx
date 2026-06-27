import { useState, useEffect, useCallback } from "react";
import { invoke } from "@/lib/tauri";
import { cn } from "@/lib/utils";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import type { ClipboardItem } from "@/types/clipboard";
import {
  Star, Mic, Type, Image as ImageIcon, FileText,
  LayoutGrid, Search, Trash2,
} from "lucide-react";

const PAGE_SIZE = 50;
const TABS = [
  { value: "all", icon: LayoutGrid, label: "全部" },
  { value: "asr", icon: Mic, label: "语音" },
  { value: "text", icon: Type, label: "文本" },
  { value: "image", icon: ImageIcon, label: "图片" },
  { value: "file", icon: FileText, label: "文件" },
  { value: "favorite", icon: Star, label: "收藏" },
] as const;

export default function ClipboardPanel({ showToast }: { showToast: (msg: string) => void }) {
  const [items, setItems] = useState<ClipboardItem[]>([]);
  const [total, setTotal] = useState(0);
  const [filter, setFilter] = useState("all");
  const [search, setSearch] = useState("");
  const [selectedIds, setSelectedIds] = useState<Set<number>>(new Set());
  const [loading, setLoading] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);

  const debouncedSearch = useDebouncedValue(search, 300);

  const fetchData = useCallback(async () => {
    setLoading(true);
    try {
      const result = await invoke<ClipboardItem[]>("query_clipboard_history", {
        filter, search: debouncedSearch || null, page: 1, size: PAGE_SIZE,
      });
      setItems(result);
      const count = await invoke<number>("clipboard_stats");
      setTotal(count);
    } catch (e) {
      showToast("加载失败：" + e);
    }
    setLoading(false);
  }, [filter, debouncedSearch, showToast]);

  useEffect(() => { fetchData(); }, [fetchData]);
  useTauriEvent("clipboard://changed", fetchData);

  const toggleSelect = (id: number) => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      next.has(id) ? next.delete(id) : next.add(id);
      return next;
    });
  };

  const toggleSelectAll = (checked: boolean) => {
    setSelectedIds(checked ? new Set(items.map((i) => i.id)) : new Set());
  };

  const handleDelete = async () => {
    if (selectedIds.size === 0) return;
    if (!confirmDelete) {
      setConfirmDelete(true);
      setTimeout(() => setConfirmDelete(false), 3000);
      return;
    }
    try {
      for (const id of selectedIds) {
        await invoke("delete_clipboard_item", { id });
      }
      showToast(`已删除 ${selectedIds.size} 条`);
      setSelectedIds(new Set());
      setConfirmDelete(false);
      fetchData();
    } catch (e) {
      showToast("删除失败：" + e);
    }
  };

  const toggleFavorite = async (id: number, e: React.MouseEvent) => {
    e.stopPropagation();
    try {
      await invoke("toggle_clipboard_favorite", { id });
      fetchData();
    } catch (e) {
      console.error(e);
    }
  };

  const allChecked = items.length > 0 && selectedIds.size === items.length;
  const indeterminate = selectedIds.size > 0 && selectedIds.size < items.length;

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
            onChange={(e) => toggleSelectAll(e.target.checked)}
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
            confirmDelete
              ? "border-red-600 text-white bg-red-600"
              : "border-red-500 text-red-500 hover:bg-red-50",
            selectedIds.size === 0 && "opacity-40 cursor-not-allowed",
          )}
          disabled={selectedIds.size === 0}
          onClick={handleDelete}
        >
          <Trash2 className="w-3.5 h-3.5" />
          {confirmDelete ? "确认删除" : "删除选中"}
        </button>
      </div>

      {/* Filter + Search */}
      <div className="flex items-center gap-3 py-3">
        <div className="flex items-center gap-1">
          {TABS.map(({ value: v, icon: Icon, label }) => (
            <button
              key={v}
              title={label}
              className={cn(
                "flex items-center justify-center px-2.5 py-1.5 rounded-md transition-colors",
                filter === v
                  ? "bg-primary text-primary-foreground"
                  : "text-muted-foreground hover:text-foreground hover:bg-accent",
              )}
              onClick={() => setFilter(v)}
            >
              <Icon className="w-4 h-4" />
              {v === "all" && <span className="ml-1 text-xs">{label}</span>}
            </button>
          ))}
        </div>
        <div className="flex-1 flex items-center gap-2 px-3 py-1.5 bg-muted rounded-md border border-border">
          <Search className="w-4 h-4 text-muted-foreground" />
          <input
            type="text"
            placeholder="搜索..."
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            className="flex-1 bg-transparent text-sm outline-none placeholder:text-muted-foreground/60"
          />
        </div>
      </div>

      {/* List */}
      <div className="flex-1 overflow-y-auto thin-scrollbar -mx-1 px-1">
        {loading ? (
          <div className="text-center py-8 text-muted-foreground text-sm">加载中...</div>
        ) : items.length === 0 ? (
          <div className="text-center py-12 text-muted-foreground text-sm">暂无记录</div>
        ) : (
          <div className="flex flex-col">
            {items.map((item) => {
              const Icon = item.source === "asr" ? Mic
                : item.item_type === "image" ? ImageIcon
                : item.item_type === "file" ? FileText
                : Type;
              const isVoice = item.source === "asr";
              const isSelected = selectedIds.has(item.id);

              return (
                <div
                  key={item.id}
                  className={cn(
                    "group flex items-start gap-2.5 px-3 py-2.5 border-b border-border/40 transition-colors cursor-pointer",
                    isSelected ? "bg-accent" : "hover:bg-accent/50",
                  )}
                  onClick={() => toggleSelect(item.id)}
                >
                  <input
                    type="checkbox"
                    className="w-4 h-4 mt-0.5 flex-shrink-0"
                    checked={isSelected}
                    onChange={(e) => { e.stopPropagation(); toggleSelect(item.id); }}
                    onClick={(e) => e.stopPropagation()}
                  />
                  <Icon className={cn(
                    "w-4 h-4 mt-0.5 flex-shrink-0",
                    isVoice ? "text-voice" : "text-muted-foreground",
                  )} />
                  <div className="flex-1 min-w-0">
                    {item.item_type === "image" && item.image_meta ? (
                      <div className="text-sm text-muted-foreground">
                        图片 {item.image_meta.width}×{item.image_meta.height}
                      </div>
                    ) : item.item_type === "file" ? (
                      <div className="text-sm text-muted-foreground truncate">
                        {formatFilePaths(item.content, item.file_meta?.file_count)}
                      </div>
                    ) : (
                      <p className="text-sm leading-relaxed text-foreground/90 break-words line-clamp-2">{item.content}</p>
                    )}
                    {isVoice && item.asr_meta && (
                      <div className="flex items-center gap-2 mt-1">
                        <span className="text-[10px] text-voice/70 font-medium">{item.asr_meta.engine}</span>
                      </div>
                    )}
                    <span className="text-[10px] text-muted-foreground/50">{item.created_at}</span>
                  </div>
                  <button
                    className={cn(
                      "p-1 transition-opacity",
                      item.is_favorite ? "opacity-100" : "opacity-0 group-hover:opacity-60 hover:!opacity-100",
                    )}
                    onClick={(e) => toggleFavorite(item.id, e)}
                  >
                    <Star className={cn("w-4 h-4", item.is_favorite ? "fill-amber-400 text-amber-400" : "text-muted-foreground")} />
                  </button>
                </div>
              );
            })}
          </div>
        )}
      </div>

      {/* Footer */}
      <div className="flex items-center justify-between py-2 border-t border-border text-xs text-muted-foreground">
        <span>共 {total} 条{filter !== "all" ? `（当前：${TABS.find(t => t.value === filter)?.label}）` : ""}</span>
        <span>显示最近 {items.length} 条</span>
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

function formatFilePaths(content: string, count?: number): string {
  try {
    const paths: string[] = JSON.parse(content);
    const display = paths.slice(0, 3).map((raw) => {
      const path = raw.replace(/^file:\/\//, "");
      const parts = path.split("/").filter(Boolean);
      return "…/" + parts.slice(-2).join("/");
    });
    if (paths.length > 3) return display.join("  ") + `  +${paths.length - 3}`;
    return display.join("  ") + (count ? ` (${count})` : "");
  } catch {
    return count ? `${count} 个文件` : "文件";
  }
}
