import { useState, useCallback } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@/lib/tauri";
import { useClipboardHistory } from "@/hooks/useClipboardHistory";
import FilterTabs from "./FilterTabs";
import SearchBar from "./SearchBar";
import ClipboardItemRow from "./ClipboardItem";
import { Pin, Trash2, X } from "lucide-react";
import { cn } from "@/lib/utils";

export default function Clipboard() {
  const [filter, setFilter] = useState("all");
  const [search, setSearch] = useState("");
  const [pinned, setPinned] = useState(false);

  const { items, total, refresh } = useClipboardHistory(filter, search);

  const handleClear = useCallback(async () => {
    if (!confirm("清空所有非收藏的历史记录？")) return;
    try {
      await invoke("clear_clipboard_history", { keepFavorite: true });
      refresh();
    } catch (e) {
      console.error(e);
    }
  }, [refresh]);

  const togglePin = useCallback(() => {
    setPinned((p) => !p);
    // TODO: set always_on_top via Tauri
  }, []);

  return (
    <div
      className="flex flex-col h-screen bg-background text-foreground select-none"
      data-tauri-drag-region
    >
      {/* Title bar */}
      <div className="flex items-center justify-between px-3 py-1.5" data-tauri-drag-region>
        <button
          className="p-1 rounded hover:bg-accent text-muted-foreground"
          onClick={() => getCurrentWindow().hide()}
          title="关闭"
        >
          <X className="w-3.5 h-3.5" />
        </button>
        <span className="text-xs text-muted-foreground">剪贴板历史</span>
        <button
          className={cn("p-1 rounded hover:bg-accent", pinned && "text-primary")}
          onClick={togglePin}
          title="置顶"
        >
          <Pin className="w-3.5 h-3.5" />
        </button>
      </div>

      <SearchBar value={search} onChange={setSearch} />
      <FilterTabs value={filter} onChange={setFilter} />

      {/* List */}
      <div className="flex-1 overflow-y-auto">
        {items.length === 0 ? (
          <div className="flex items-center justify-center h-full text-sm text-muted-foreground">
            暂无记录
          </div>
        ) : (
          items.map((item) => (
            <ClipboardItemRow key={item.id} item={item} onChanged={refresh} />
          ))
        )}
      </div>

      {/* Footer */}
      <div className="flex items-center justify-between px-3 py-1.5 border-t border-border text-xs text-muted-foreground">
        <span>共 {total} 条</span>
        <button
          className="flex items-center gap-1 hover:text-destructive transition-colors"
          onClick={handleClear}
        >
          <Trash2 className="w-3 h-3" />
          清空
        </button>
      </div>
    </div>
  );
}
